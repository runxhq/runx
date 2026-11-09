// Step handlers implement the concrete approval, agent, nested-workflow, tool,
// and sub-skill acts behind the typed dispatch boundary.

mod host_resolution;
mod load;
mod output;

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::path::Path;
use std::time::Instant;

use output::{build_ephemeral_step_output, build_step_output_projection, contract_output_claim};

use runx_contracts::{
    ApprovalGate, CallAct, ExecutionEvent, JsonObject, JsonValue, ProvenanceEntry, Receipt,
    Reference, ResolutionRequest, ResolutionResponse, ResolutionResponseActor, RunOutcome,
};
use runx_core::state_machine::StepAdmissionWitness;
use runx_parser::{AskGate, SkillArtifactContract, StepKind, ToolTarget, WorkflowStep};

use self::host_resolution::resolve_step_approval;
pub(super) use self::load::resolve_loaded;
use super::super::graph::{
    LoadedStepSkill, StepSkillLoadOptions, bind_run_inputs, load_step_skill,
};
use super::super::skill_context::load_context_skills;
use super::admission::{
    EffectReceiptContext, StepAuthorityContext, enforce_step_authority_admission,
    finalize_effect_output_before_success, persist_effect_state_for_step, prepare_effect_execution,
    prepare_effect_output_before_gate,
};
use super::turn::{OpenTurn, TurnScope};
use super::{
    Runtime, StepRun, WorkflowRun, workflow_run_ephemeral_result, workflow_run_result_artifacts,
    workflow_run_skill_output,
};
use crate::RuntimeError;
use crate::adapter::{
    BorrowedSkillAdapter, EphemeralValue, Executable, InvocationOutput, SkillAdapter,
    SkillInvocation,
};
use crate::agent_contract::agent_contract_checks_with_artifacts;
use crate::agent_invocation::turn_resolution_request;
use crate::approval::ApprovalResolution;
use crate::effects::EffectStepRequest;
use crate::effects::ResolvedEffectTarget;
use crate::execution::outcome::agent_answer_outcome_or_completed;
use crate::execution::output_projection::{StepOutputProjection, project_step_claim};
use crate::host::Host;
use crate::observations::ContractChecks;
use crate::output_contract::method_contract_checks;
use crate::receipts::{RuntimeReceiptSignaturePolicy, StepSeal, StepSealEnding, seal_step};
use crate::services::merge_inferred_tool_roots;

struct RegularSkillStepOutput {
    output: InvocationOutput,
    projection: StepOutputProjection,
    ephemeral_contract: JsonObject,
    receipt_lineage: StepReceiptLineage,
}

#[derive(Default)]
struct StepReceiptLineage {
    direct_children: Vec<Receipt>,
    descendants: Vec<Receipt>,
}

impl StepReceiptLineage {
    fn from_workflow(run: WorkflowRun) -> Self {
        let WorkflowRun { steps, receipt, .. } = run;
        let mut descendants = Vec::new();
        for mut step in steps {
            descendants.append(&mut step.nested_receipts);
            descendants.push(step.receipt);
        }
        Self {
            direct_children: vec![receipt],
            descendants,
        }
    }

    /// An agent turn's lineage: each completed call's receipt is a child,
    /// in order, and the receipts it carried travel with the step.
    fn of_turn(calls: &[&StepRun]) -> Self {
        Self {
            direct_children: calls.iter().map(|call| call.receipt.clone()).collect(),
            descendants: calls
                .iter()
                .flat_map(|call| call.nested_receipts.iter().cloned())
                .collect(),
        }
    }

    fn into_nested_receipts(mut self) -> Vec<Receipt> {
        self.descendants.append(&mut self.direct_children);
        self.descendants
    }
}

pub(super) struct StepRunRequest<'a, A> {
    pub(super) runtime: &'a Runtime<A>,
    pub(super) workflow_dir: &'a Path,
    pub(super) workflow_name: &'a str,
    pub(super) step: &'a WorkflowStep,
    pub(super) attempt: u32,
    pub(super) inputs: JsonObject,
    /// The act a skill step gives its child run, which the child reads as
    /// `$run.act`.
    pub(super) child_act: Option<CallAct>,
    pub(super) provenance: Vec<ProvenanceEntry>,
    pub(super) admission_refs: Vec<Reference>,
    /// The floor holds the step's write under `effects: ask`: its one gate
    /// carries that reason.
    pub(super) held_by_effects: bool,
    pub(super) host: &'a mut dyn Host,
}

struct StepHandlerCtx<'a, A> {
    runtime: &'a Runtime<A>,
    workflow_dir: &'a Path,
    workflow_name: &'a str,
    step: &'a WorkflowStep,
    attempt: u32,
    inputs: JsonObject,
    child_act: Option<CallAct>,
    provenance: Vec<ProvenanceEntry>,
    admission_refs: Vec<Reference>,
    host: &'a mut dyn Host,
    authority: Option<StepAuthorityContext>,
    loaded_skill: Option<LoadedStepSkill>,
}

struct RegularSkillSeal<'a, A> {
    runtime: &'a Runtime<A>,
    workflow_dir: &'a Path,
    workflow_name: &'a str,
    step: &'a WorkflowStep,
    attempt: u32,
    skill_name: String,
    authority: Option<&'a StepAuthorityContext>,
    admission_refs: Vec<Reference>,
    /// How the step closes when its output does not say: a skill step whose
    /// child stopped short closes as its child sealed.
    closure: Option<StepSealEnding>,
    /// Why a guard or an authority refused the step's child run, when one
    /// did.
    refusal: Option<JsonObject>,
}

pub(super) fn output_error(run: &StepRun) -> String {
    run.outcome
        .failure_message()
        .unwrap_or_else(|| "step failed without diagnostics".to_owned())
}

// Function rationale: step execution is one linear admit/run/seal sequence; splitting
// it would scatter the ordering invariants between admission, invocation, and receipt sealing.
pub(super) fn run_step_with_inputs<A>(
    request: StepRunRequest<'_, A>,
) -> Result<StepRun, RuntimeError>
where
    A: SkillAdapter,
{
    run_step_with_optional_loaded_skill(request, None)
}

pub(super) fn run_step_with_loaded_skill_inputs<A>(
    request: StepRunRequest<'_, A>,
    loaded_skill: LoadedStepSkill,
) -> Result<StepRun, RuntimeError>
where
    A: SkillAdapter,
{
    run_step_with_optional_loaded_skill(request, Some(loaded_skill))
}

// Function rationale: this is the single routing point that preserves
// authority admission, effect preparation, and dispatch order.
fn run_step_with_optional_loaded_skill<A>(
    request: StepRunRequest<'_, A>,
    loaded_skill: Option<LoadedStepSkill>,
) -> Result<StepRun, RuntimeError>
where
    A: SkillAdapter,
{
    let StepRunRequest {
        runtime,
        workflow_dir,
        workflow_name,
        step,
        attempt,
        inputs,
        child_act,
        provenance,
        admission_refs,
        held_by_effects,
        host,
    } = request;
    // An operation call is admitted against the packages it calls from.
    let chain = if step.operation().is_some() {
        runtime.options.chain_at(workflow_dir)?
    } else {
        Vec::new()
    };
    let effect_request = EffectStepRequest {
        step,
        target: ResolvedEffectTarget {
            tool: step.catalog_tool(),
        },
        inputs: &inputs,
        env: &runtime.options.env,
        workflow_dir,
        run_id: runtime.options.run.run_id(),
        run_authority: runtime.options.run_authority.as_ref(),
        held_by_effects,
        chain: &chain,
    };
    let authority = enforce_step_authority_admission(effect_request, &runtime.options.effects)?;
    let authority =
        prepare_effect_execution(effect_request, authority, host, &runtime.options.effects)?;
    run_registered_step(StepHandlerCtx {
        runtime,
        workflow_dir,
        workflow_name,
        step,
        attempt,
        inputs,
        child_act,
        provenance,
        admission_refs,
        host,
        authority,
        loaded_skill,
    })
}

// A child skill always runs its method's graph, nested under this step, with
// its declared inputs materialized from the step's.
fn run_loaded_skill_step<A>(
    skill: LoadedStepSkill,
    request: StepHandlerCtx<'_, A>,
) -> Result<StepRun, RuntimeError>
where
    A: SkillAdapter,
{
    let inputs = crate::input_contract::materialize_nested_method_inputs(
        &skill.method.inputs,
        &skill.method.input_constraints,
        &request.inputs,
    )
    .map_err(crate::input_contract::InputContractError::into_runtime_error)?;
    // A method the registry sells is bought from its seller, never run here.
    if let Some(paid) = request.step.skill().and_then(|target| {
        crate::kernel::purchase::PaidMethod::reached(
            &target.skill,
            skill.version.as_deref(),
            &skill.method.name,
            skill.offer.as_ref(),
        )
    }) {
        // A purchase moves money at a provider, which `effects: none` never
        // lets a run do.
        if crate::kernel::authority::step_effects(
            request.runtime.options.run_authority.as_ref(),
            request.step,
        ) == runx_contracts::Effects::None
        {
            return Err(RuntimeError::AuthorityDenied {
                verb: runx_contracts::AuthorityVerb::Write,
                step_id: request.step.id.clone(),
                reason: format!(
                    "the run's effects are none, so it changes nothing: buying {}#{} is refused",
                    paid.skill_id, paid.method
                ),
            });
        }
        return run_purchase_step(paid, StepHandlerCtx { inputs, ..request });
    }
    let mut env = request.runtime.options.env.clone();
    merge_inferred_tool_roots(&mut env, &skill.directory);
    crate::execution_environment::resolve_declared_environment(
        &skill.requirements,
        &env,
        &skill.directory,
    )?;
    run_nested_workflow_skill_step(StepHandlerCtx { inputs, ..request }, skill, env)
}

/// A call that reaches a method the registry sells: the kernel buys it
/// (S10). The step's output is the purchase as its buyer reads it, and the
/// act settles with the step's receipt.
fn run_purchase_step<A>(
    paid: crate::kernel::purchase::PaidMethod,
    request: StepHandlerCtx<'_, A>,
) -> Result<StepRun, RuntimeError>
where
    A: SkillAdapter,
{
    use crate::kernel::purchase::{Bought, PurchaseCall};
    let StepHandlerCtx {
        runtime,
        workflow_dir,
        workflow_name,
        step,
        attempt,
        inputs,
        admission_refs,
        host,
        authority,
        ..
    } = request;
    let started = Instant::now();
    let address = runtime.options.run.address(&step.id);
    let hosted = crate::kernel::operation::hosted::HostedClient::default();
    let bought = crate::kernel::purchase::buy(
        &PurchaseCall {
            paid: &paid,
            inputs: &inputs,
            run_id: runtime.options.run.run_id(),
            step_id: &step.id,
            address: &address,
            limit: runtime
                .options
                .run_authority
                .as_ref()
                .and_then(|authority| authority.max_spend_micro_usd),
            env: &runtime.options.env,
            cwd: workflow_dir,
            hosted: &hosted,
        },
        host,
    )?;
    let projection = crate::adapter_pipeline::AdapterProjection::from_started(started);
    let observations = crate::observations::Observations::default();
    let (output, settlement) = match bought {
        Bought::Fulfilled {
            purchase,
            settlement,
        } => {
            let value = serde_json::to_value(&*purchase)
                .and_then(serde_json::from_value)
                .map_err(|source| RuntimeError::json("reporting the purchase", source))?;
            (
                projection.runtime_output(
                    crate::adapter::InvocationStatus::Success,
                    value,
                    None,
                    observations,
                ),
                Some(settlement),
            )
        }
        Bought::Refused { code, reason } => (
            projection.failure(format!("{code}: {reason}"), observations),
            None,
        ),
    };
    let mut run = seal_effect_step(
        EffectStepSeal {
            runtime,
            workflow_dir,
            workflow_name,
            step,
            attempt,
            admission_refs,
            authority: authority.as_ref(),
            skill: paid.skill_id.clone(),
            call: None,
        },
        output,
        None,
    )?;
    if let Some(settlement) = settlement {
        run.settlement = Some(settlement.sealed(run.receipt.id.as_str().to_owned()));
    }
    Ok(run)
}

fn run_nested_workflow_skill_step<A>(
    request: StepHandlerCtx<'_, A>,
    skill: LoadedStepSkill,
    env: std::collections::BTreeMap<String, String>,
) -> Result<StepRun, RuntimeError>
where
    A: SkillAdapter,
{
    let skill_name = skill.method.name.clone();
    let admission_refs = request.admission_refs.clone();
    let chain = child_chain(&request, &skill)?;
    let run = execute_nested_workflow(
        request.runtime,
        request.host,
        NestedWorkflow {
            step: request.step,
            skill: &skill_name,
            graph: skill.method.graph,
            inputs: &request.inputs,
            act: request.child_act.as_ref(),
            provenance: &request.provenance,
            env: &env,
            skill_directory: &skill.directory,
            admitted: skill.admitted.as_ref(),
            chain,
        },
    )?;
    // The step closes as its child sealed: a child that stopped short
    // stops its step with the same closure, carrying every receipt it made.
    let child_seal = &run.receipt.seal;
    let closure = (child_seal.outcome != RunOutcome::Completed).then_some(StepSealEnding {
        outcome: child_seal.outcome,
        reason: child_seal.reason,
    });
    let result = super::workflow_terminal_result(&run)?;
    let ephemeral_result = workflow_run_ephemeral_result(&run);
    let mut output = workflow_run_skill_output(&result, &run)?;
    let mut projection = build_step_output_projection(request.step, &output, None, None)?;
    adopt_nested_workflow_result(
        &result,
        &workflow_run_result_artifacts(&run),
        &mut projection,
    );
    let mut ephemeral_contract = JsonObject::new();
    if let JsonValue::Object(ephemeral) = ephemeral_result {
        ephemeral_contract.extend(ephemeral);
    }
    let effect_claim = contract_output_claim(&projection);
    prepare_effect_output_before_gate(
        request.step,
        request.authority.as_ref(),
        effect_claim,
        &mut output,
        &request.runtime.options.effects,
    )?;
    if output.succeeded() {
        // The packets the step declares are checked against the artifacts it
        // holds, as its parent adopted them; the parent declares them, so
        // they resolve from the parent's package.
        output.observations.contract = crate::output_contract::artifact_contract_checks(
            &projection.outputs,
            request.step.artifacts.as_ref(),
            request.workflow_dir,
        )?;
    }
    let refusal = run.refusal.clone();
    let receipt_lineage = StepReceiptLineage::from_workflow(run);
    seal_regular_skill_step(
        RegularSkillSeal {
            runtime: request.runtime,
            workflow_dir: request.workflow_dir,
            workflow_name: request.workflow_name,
            step: request.step,
            attempt: request.attempt,
            skill_name,
            authority: request.authority.as_ref(),
            admission_refs,
            closure,
            refusal,
        },
        RegularSkillStepOutput {
            output,
            projection,
            ephemeral_contract,
            receipt_lineage,
        },
    )
}

/// A child skill's graph and what it runs with.
struct NestedWorkflow<'a> {
    /// The parent step the child runs as.
    step: &'a WorkflowStep,
    skill: &'a str,
    graph: runx_parser::Workflow,
    inputs: &'a JsonObject,
    /// The act the parent step gives the child.
    act: Option<&'a CallAct>,
    /// The parent step's provenance, carried to the steps that receive the
    /// same inputs.
    provenance: &'a [ProvenanceEntry],
    env: &'a std::collections::BTreeMap<String, String>,
    skill_directory: &'a Path,
    /// The packages a load bound for the child, which it executes in place
    /// of its parent's.
    admitted: Option<&'a crate::skill_package::AdmittedPackages>,
    /// The child run's chain: its parent's, then the package the step
    /// reached.
    chain: Vec<crate::identity::ChainLink>,
}

/// The chain of the child run `skill` starts from the step in `request`: the
/// parent run's, then the child's package as the step's reference reached it.
fn child_chain<A>(
    request: &StepHandlerCtx<'_, A>,
    skill: &LoadedStepSkill,
) -> Result<Vec<crate::identity::ChainLink>, RuntimeError> {
    let options = &request.runtime.options;
    let mut chain = options.chain_at(request.workflow_dir)?;
    let package = match &skill.admitted {
        Some(admitted) => admitted.get(&skill.directory)?,
        None => options.package(&skill.directory)?,
    };
    let reference = request
        .step
        .skill()
        .map_or_else(String::new, |target| target.skill.clone());
    let signed = skill.registry.as_ref().map(|registry| &registry.signed);
    let refused =
        |reason: String| crate::execution::runner::identity_refused(&skill.directory, &reason);
    let Some(parent) = chain.last() else {
        // A step no package runs reaches a package that binds by itself.
        let identity = match signed {
            Some(signed) => crate::identity::PackageIdentity::Signed(signed.clone()),
            None => crate::identity::skill_identity(&package, &options.env).map_err(refused)?,
        };
        return Ok(vec![
            crate::identity::ChainLink::of(&identity, &package).map_err(refused)?,
        ]);
    };
    let link = parent
        .child(&reference, &package, signed, &options.env)
        .map_err(refused)?;
    if link.root != parent.root {
        chain.push(link);
    }
    Ok(chain)
}

/// Run the child run a nested skill step runs as, or continue it from its
/// record. A child sealed before its parent step was recorded returns its
/// sealed run; a suspension inside it names the child run that holds it.
fn execute_nested_workflow<A>(
    runtime: &Runtime<A>,
    host: &mut dyn Host,
    nested: NestedWorkflow<'_>,
) -> Result<WorkflowRun, RuntimeError>
where
    A: SkillAdapter,
{
    let inherited =
        crate::execution::graph::inherited_step_provenance(&nested.graph, nested.provenance);
    let log = runtime.options.run.child(&nested.step.id)?;
    let graph = bind_run_inputs(nested.graph, nested.inputs, nested.act, log.run_id())?;
    if log.load()?.is_none()
        && let Some(reason) = super::child_depth_refusal(&runtime.options)
    {
        return Err(RuntimeError::AuthorityDenied {
            verb: runx_contracts::AuthorityVerb::Execute,
            step_id: nested.step.id.clone(),
            reason,
        });
    }
    let existing = log.start_child(
        runtime.options.run.run_id(),
        &nested.step.id,
        nested.skill,
        nested.inputs,
        nested.act,
        &runtime.options.clock.now(),
    )?;
    if let Some(record) = existing.filter(|record| record.sealed.is_some()) {
        return super::sealed_workflow_run(graph, record);
    }
    let mut child_options = runtime.options.as_ref().clone();
    // A child holds what its parent step does, narrowed by its own method.
    child_options.run_authority = crate::RunAuthority::of_method(
        crate::RunAuthority::of_step(child_options.run_authority.take(), nested.step),
        &graph,
    );
    child_options.env = nested.env.clone();
    child_options.run = log.clone();
    if let Some(admitted) = nested.admitted {
        child_options.packages = Some(admitted.clone());
    }
    child_options.chain = nested.chain;
    let child_adapter: Box<dyn SkillAdapter + '_> =
        Box::new(BorrowedSkillAdapter::new(&runtime.configured_adapter));
    let child_runtime = Runtime::with_native_services(
        child_adapter,
        child_options,
        runtime.javascript.clone(),
        runtime.local_artifacts.clone(),
    )
    .with_inherited_provenance(inherited);
    match child_runtime.run_workflow_with_host(nested.skill_directory, graph.clone(), host) {
        Ok(run) => {
            log.record_sealed(&run.receipt, None)?;
            Ok(run)
        }
        Err(RuntimeError::ResolutionPending {
            step_id,
            reason,
            request,
            ..
        }) => Err(RuntimeError::ResolutionPending {
            step_id,
            reason,
            request,
            child: Some(log.run_id().to_owned()),
        }),
        // A child that stopped short seals as a run of it would, over every
        // step it ran, and its parent step closes over it. What leaves the
        // child open leaves its parent open.
        Err(error) => {
            let stopped = child_runtime.seal_stop(graph, error, host)?;
            log.record_sealed(&stopped.run.receipt, Some(&stopped.result))?;
            Ok(stopped.run)
        }
    }
}

/// A nested graph exposes the same canonical public result that a top-level
/// graph returns, and which of its outputs are artifacts. This keeps
/// composition and direct invocation on one output contract without
/// re-reading the nested graph's diagnostic step state.
fn adopt_nested_workflow_result(
    result: &JsonValue,
    artifacts: &BTreeSet<String>,
    projection: &mut StepOutputProjection,
) {
    let JsonValue::Object(result) = result else {
        return;
    };
    for (name, value) in result {
        projection.outputs.insert(name.clone(), value.clone());
        if artifacts.contains(name) {
            projection.artifact_outputs.insert(name.clone());
        } else {
            projection.artifact_outputs.remove(name);
        }
    }
}

fn invoke_regular_skill_step<A>(
    runtime: &Runtime<A>,
    step: &WorkflowStep,
    invocation: SkillInvocation,
    extra_artifacts: Option<&SkillArtifactContract>,
    authority: Option<&StepAuthorityContext>,
) -> Result<RegularSkillStepOutput, RuntimeError>
where
    A: SkillAdapter,
{
    let skill_name = invocation.skill_name.clone();
    let raw_output = invocation.outputs.clone();
    let skill_directory = invocation.skill_directory.clone();
    let mut output = if matches!(invocation.executable, Executable::Worker(_)) {
        // A worker runs the modules its run admitted.
        let package = runtime.options.package(&invocation.skill_directory)?;
        runtime
            .javascript
            .invoke_with_artifacts(invocation, &package, &runtime.local_artifacts)?
    } else {
        if matches!(invocation.executable, Executable::Process(_)) {
            runtime
                .options
                .confirm_on_disk(&invocation.skill_directory, &step.id)?;
        }
        runtime.configured_adapter.invoke(invocation)?
    };
    let provisional_projection =
        build_step_output_projection(step, &output, raw_output.as_ref(), extra_artifacts)?;
    let effect_claim = contract_output_claim(&provisional_projection);
    prepare_effect_output_before_gate(
        step,
        authority,
        effect_claim,
        &mut output,
        &runtime.options.effects,
    )?;
    // An effect may remove transient provider material before sealing. Rebuild
    // the projection so receipts, durable replay state, and downstream context
    // all observe the same public output.
    let projection =
        build_step_output_projection(step, &output, raw_output.as_ref(), extra_artifacts)?;
    let ephemeral_contract =
        build_ephemeral_step_output(step, &output, raw_output.as_ref(), extra_artifacts);
    if output.succeeded() {
        output.observations.contract = method_contract_checks(
            &skill_name,
            &output.value,
            raw_output.as_ref(),
            step.artifacts.as_ref().or(extra_artifacts),
            &skill_directory,
        )?;
    }
    Ok(RegularSkillStepOutput {
        output,
        projection,
        ephemeral_contract,
        receipt_lineage: StepReceiptLineage::default(),
    })
}

// Function rationale: sealing keeps the declared contract, effect evidence, and
// receipt construction consistent.
fn seal_regular_skill_step<A>(
    context: RegularSkillSeal<'_, A>,
    regular: RegularSkillStepOutput,
) -> Result<StepRun, RuntimeError>
where
    A: SkillAdapter,
{
    let RegularSkillStepOutput {
        mut output,
        projection,
        ephemeral_contract,
        receipt_lineage,
    } = regular;
    let effect_claim = contract_output_claim(&projection);
    let seal_authority = context
        .authority
        .map(|authority| authority.authority(&context.runtime.options.effects))
        .transpose()?
        .unwrap_or_default();
    let receipt = seal_executed_step(
        StepSeal {
            scope: context.runtime.scope(context.workflow_name),
            step_id: &context.step.id,
            attempt: context.attempt,
            output: &output,
            claim: &projection.outputs,
            created_at: &context.runtime.options.clock.now(),
            authority: seal_authority,
            call: None,
            admission_refs: context.admission_refs,
            child_receipts: &receipt_lineage.direct_children,
            descendant_receipts: &receipt_lineage.descendants,
            closure: context.closure,
        },
        context.runtime.options.signature_policy(),
    )?;
    finalize_effect_output_before_success(EffectReceiptContext {
        step: context.step,
        workflow_dir: context.workflow_dir,
        authority: context.authority,
        claim: effect_claim,
        output: &mut output,
        receipt: &receipt,
        env: &context.runtime.options.env,
        signature_policy: context.runtime.options.signature_policy(),
        effects: &context.runtime.options.effects,
    })
    .map_err(|source| RuntimeError::engine("finalizing a sealed provider effect", source))?;
    let settlement = persist_effect_state_for_step(EffectReceiptContext {
        step: context.step,
        workflow_dir: context.workflow_dir,
        authority: context.authority,
        claim: effect_claim,
        output: &mut output,
        receipt: &receipt,
        env: &context.runtime.options.env,
        signature_policy: context.runtime.options.signature_policy(),
        effects: &context.runtime.options.effects,
    })
    .map_err(|source| RuntimeError::engine("persisting a sealed provider effect", source))?;
    // The authority witness is sealed centrally in run_registered_step; the seal
    // path records a neutral witness and uses `authority` only for effect output
    // finalization above.
    let admission_witness =
        StepAdmissionWitness::local_runtime(&context.step.id, receipt.id.as_str());
    Ok(StepRun {
        step_id: context.step.id.clone(),
        attempt: context.attempt,
        skill: context.skill_name,
        method: step_runner(context.step),
        contract: projection.outputs,
        artifact_outputs: projection.artifact_outputs,
        ephemeral_contract: EphemeralValue::from_value(JsonValue::Object(ephemeral_contract)),
        outcome: output.into(),
        receipt,
        nested_receipts: receipt_lineage.into_nested_receipts(),
        refusal: context.refusal,
        admission_witness,
        settlement,
    })
}

fn loaded_skill_or_load(
    loaded_skill: Option<LoadedStepSkill>,
    options: &super::RuntimeOptions,
    workflow_dir: &Path,
    step: &WorkflowStep,
) -> Result<LoadedStepSkill, RuntimeError> {
    loaded_skill.map_or_else(
        || load_step_skill(workflow_dir, step, StepSkillLoadOptions::of_run(options)),
        Ok,
    )
}

/// The child method a skill step names, if any.
fn step_runner(step: &WorkflowStep) -> Option<String> {
    step.skill().and_then(|target| target.method.clone())
}

fn run_registered_step<A>(request: StepHandlerCtx<'_, A>) -> Result<StepRun, RuntimeError>
where
    A: SkillAdapter,
{
    // Every registered step is admitted centrally (enforce_step_authority_admission,
    // upstream) and sealed centrally here: this is the single place a step's
    // admission witness records which authority admitted the act, or falls back to a
    // local-runtime witness when none was admitted. Handlers produce the output and
    // receipt; they never set the authority witness, so a new step type cannot
    // regress the uniform-governance invariant. See `docs/governance-invariant.md`
    // for the full admit -> credentials -> execution -> seal contract.
    let step = request.step;
    let authority = request.authority.clone();
    let mut run = match &step.kind {
        StepKind::Ask(ask) => run_approval_step(request, ask)?,
        StepKind::Agent(_) => run_agent_task(request)?,
        StepKind::Tool(ToolTarget::Catalog(_)) => run_tool_step(request)?,
        StepKind::Tool(ToolTarget::Worker(_) | ToolTarget::Process(_)) => {
            run_executable_tool_step(request)?
        }
        StepKind::Operation(_) => run_operation_step(request)?,
        StepKind::Skill(_) => run_subskill_step(request)?,
        StepKind::Load(load) => load::run_load_step(request, load)?,
    };
    run.admission_witness =
        step_admission_witness(&step.id, run.receipt.id.as_str(), authority.as_ref());
    Ok(run)
}

fn run_subskill_step<A>(mut request: StepHandlerCtx<'_, A>) -> Result<StepRun, RuntimeError>
where
    A: SkillAdapter,
{
    let skill = loaded_skill_or_load(
        request.loaded_skill.take(),
        &request.runtime.options,
        request.workflow_dir,
        request.step,
    )?;
    run_loaded_skill_step(skill, request)
}

// A worker or process tool step runs through the shared projection and
// sealing path; its contract is the step's own outputs and artifacts.
fn run_executable_tool_step<A>(request: StepHandlerCtx<'_, A>) -> Result<StepRun, RuntimeError>
where
    A: SkillAdapter,
{
    let StepHandlerCtx {
        runtime,
        workflow_dir,
        workflow_name,
        step,
        attempt,
        inputs,
        child_act: _,
        provenance,
        host: _,
        authority,
        loaded_skill: _,
        admission_refs,
    } = request;
    let executable = match &step.kind {
        StepKind::Tool(ToolTarget::Worker(worker)) => Executable::Worker(worker.clone()),
        StepKind::Tool(ToolTarget::Process(process)) => Executable::Process(process.clone()),
        _ => {
            return Err(RuntimeError::StepFailed {
                step_id: step.id.clone(),
                reason: "not a worker or process tool step".to_owned(),
            });
        }
    };
    let requirements = step_requirements(&executable);
    let invocation = SkillInvocation {
        reach: crate::kernel::authority::step_reach(runtime.options.run_authority.as_ref(), step),
        historical_context: Vec::new(),
        skill_name: step.id.clone(),
        step_id: Some(step.id.clone()),
        executable,
        outputs: step.outputs.clone(),
        requirements,
        artifacts: step.artifacts.clone(),
        inputs,
        resolved_inputs: JsonObject::new(),
        current_context: Vec::new(),
        provenance,
        skill_directory: workflow_dir.to_path_buf(),
        env: runtime.options.env.clone(),
        credential_delivery: crate::credentials::CredentialDelivery::none(),
    };
    // A worker or process step may change anything: it holds the gate alone.
    let pass = runtime.options.effect_pass(false);
    let regular = invoke_regular_skill_step(runtime, step, invocation, None, authority.as_ref())?;
    drop(pass);
    seal_regular_skill_step(
        RegularSkillSeal {
            runtime,
            workflow_dir,
            workflow_name,
            step,
            attempt,
            skill_name: step.id.clone(),
            authority: authority.as_ref(),
            admission_refs,
            closure: None,
            refusal: None,
        },
        regular,
    )
}

// The close for an agent act: a resolved host response becomes the step's
// output, projection, and sealed receipt.
struct TurnStepSeal<'a> {
    workflow_name: &'a str,
    step: &'a WorkflowStep,
    attempt: u32,
    skill_name: String,
    response: ResolutionResponse,
    extra_outputs: Option<&'a JsonObject>,
    extra_artifacts: Option<&'a SkillArtifactContract>,
    contract_checks: ContractChecks,
    /// The turn's completed calls, in order.
    calls: Vec<&'a StepRun>,
}

fn seal_turn_step<A>(
    runtime: &Runtime<A>,
    request: TurnStepSeal<'_>,
) -> Result<StepRun, RuntimeError> {
    let TurnStepSeal {
        workflow_name,
        step,
        attempt,
        skill_name,
        response,
        extra_outputs,
        extra_artifacts,
        contract_checks,
        calls,
    } = request;
    let outcome = agent_answer_outcome_value(step, &response.payload)?;
    // The turn's receipt names each call the agent made, in order.
    let lineage = StepReceiptLineage::of_turn(&calls);
    // The answer was verified against its output contract whatever closure it
    // claims, so an act that declines or defers still returns its outputs. An
    // act that failed has no result.
    let projection = if outcome == RunOutcome::Failed {
        project_step_claim(JsonObject::new(), None)
    } else {
        let answer = InvocationOutput::runtime_success(response.payload.clone(), 0);
        build_step_output_projection(step, &answer, extra_outputs, extra_artifacts)?
    };
    let mut output = agent_task_output(response, &outcome)?;
    output.observations.contract = contract_checks;
    let receipt = seal_executed_step(
        StepSeal {
            scope: runtime.scope(workflow_name),
            step_id: &step.id,
            attempt,
            output: &output,
            claim: &projection.outputs,
            created_at: &runtime.options.clock.now(),
            authority: crate::receipts::SealAuthority::default(),
            call: None,
            admission_refs: Vec::new(),
            child_receipts: &lineage.direct_children,
            descendant_receipts: &lineage.descendants,
            closure: Some(StepSealEnding {
                outcome,
                reason: None,
            }),
        },
        runtime.options.signature_policy(),
    )?;
    let admission_witness = StepAdmissionWitness::local_runtime(&step.id, receipt.id.as_str());
    Ok(StepRun {
        step_id: step.id.clone(),
        attempt,
        skill: skill_name,
        method: step_runner(step),
        contract: projection.outputs,
        artifact_outputs: projection.artifact_outputs,
        ephemeral_contract: EphemeralValue::default(),
        outcome: output.into(),
        receipt,
        nested_receipts: lineage.into_nested_receipts(),
        refusal: None,
        admission_witness,
        settlement: None,
    })
}

// Function rationale: agent-task execution is one
// request/resolve/seal trust-boundary path.
fn run_agent_task<A>(request: StepHandlerCtx<'_, A>) -> Result<StepRun, RuntimeError>
where
    A: SkillAdapter,
{
    let StepHandlerCtx {
        runtime,
        workflow_dir,
        workflow_name,
        step,
        attempt,
        inputs,
        child_act: _,
        provenance,
        host,
        authority: _,
        loaded_skill: _,
        admission_refs: _,
    } = request;
    let StepKind::Agent(agent) = &step.kind else {
        return Err(RuntimeError::StepFailed {
            step_id: step.id.clone(),
            reason: "not an agent step".to_owned(),
        });
    };
    let executable = Executable::Agent(agent.clone());
    let requirements = step_requirements(&executable);
    // The act's context skills are dated by the run's admission, so the act
    // a step asks again when the run is driven again is the one it waits on.
    let admitted_at = match runtime.options.run.admitted_at()? {
        Some(admitted_at) => admitted_at,
        None => runtime.options.clock.now(),
    };
    let invocation = SkillInvocation {
        reach: crate::kernel::authority::step_reach(runtime.options.run_authority.as_ref(), step),
        historical_context: Vec::new(),
        skill_name: step.id.clone(),
        step_id: Some(step.id.clone()),
        executable,
        outputs: step.outputs.clone(),
        requirements,
        artifacts: step.artifacts.clone(),
        inputs,
        resolved_inputs: JsonObject::new(),
        current_context: load_context_skills(
            &step.id,
            workflow_dir,
            &step.context_skills,
            &runtime.options.env,
            &admitted_at,
        )?,
        provenance,
        skill_directory: workflow_dir.to_path_buf(),
        env: runtime.options.env.clone(),
        credential_delivery: crate::credentials::CredentialDelivery::none(),
    };
    let mut invocation = invocation;
    invocation
        .current_context
        .extend(runtime.options.current_context.clone());
    invocation.historical_context = runtime.options.historical_context.clone();
    let request_id = runtime.options.run.address(&step.id);
    // The act's instructions are the manual its run admitted.
    let package = runtime.options.package(&invocation.skill_directory)?;
    // The act carries why its run exists.
    let act = runtime.options.run.load()?.and_then(|record| record.act);
    let request = turn_resolution_request(&invocation, &package, act.as_ref(), request_id.clone())?;
    host.report(ExecutionEvent::ResolutionRequested {
        message: format!("agent step '{}' requested resolution", step.id),
        data: Some(resolution_event_data(step, &request)?),
    })?;
    // The act's turn is open until it is answered; the host works it here,
    // in this step's own runtime.
    let mut turn = OpenTurn::open(
        TurnScope {
            runtime,
            workflow_dir,
            workflow_name,
            step_id: &step.id,
            attempt,
            agent,
            effects: step.effects,
            reach: step.reach,
        },
        &request,
    )?;
    host.drive_turn(&mut turn)?;
    let Some(response) = host.resolve(request.clone())? else {
        return Err(RuntimeError::ResolutionPending {
            step_id: step.id.clone(),
            reason: format!("agent act {request_id} requires resolution"),
            request: Box::new(request.clone()),
            child: None,
        });
    };
    // A turn ends only once each of its calls has settled.
    if let Some(index) = turn.unsettled()? {
        return Err(RuntimeError::ContinuationRejected {
            reason: format!(
                "agent answer {request_id} was refused: call {index} of its turn has not settled"
            ),
        });
    }
    // An answer that breaks the act's output contract is refused, not sealed:
    // the turn stays open for a corrected answer. An act that failed returns
    // no outputs to check; its step fails with its reason.
    let contract_checks =
        if agent_answer_outcome_value(step, &response.payload)? == RunOutcome::Failed {
            ContractChecks::default()
        } else {
            agent_contract_checks_with_artifacts(
                &request,
                &response.payload,
                step.artifacts.as_ref(),
                workflow_dir,
            )
            .map_err(|error| match error {
                RuntimeError::SkillFailed { message, .. } => RuntimeError::ContinuationRejected {
                    reason: format!("agent answer {request_id} was refused: {message}"),
                },
                error => error,
            })?
        };
    seal_turn_step(
        runtime,
        TurnStepSeal {
            workflow_name,
            step,
            attempt,
            skill_name: format!("agent:{}", agent.task),
            response,
            extra_outputs: None,
            extra_artifacts: None,
            contract_checks,
            calls: turn.completed()?,
        },
    )
}

fn step_requirements(executable: &Executable) -> runx_contracts::ExecutionRequirements {
    let environment = match executable {
        Executable::Worker(worker) => worker.environment.clone(),
        Executable::Process(process) => process.environment.clone(),
        Executable::Agent(_) => runx_contracts::EnvironmentRequirements::default(),
    };
    // A worker that reads its input in pages reads one file: the
    // `paged_input` it declares is its `fs.read`, as a catalog tool runs with
    // the scopes it declares.
    let scopes = match executable {
        Executable::Worker(worker) if worker.paged_input.is_some() => vec!["fs.read".to_owned()],
        _ => Vec::new(),
    };
    runx_contracts::ExecutionRequirements {
        scopes,
        environment,
    }
}

// Function rationale: tool execution keeps lookup,
// invocation, and receipt sealing in one audited boundary.
fn run_tool_step<A>(request: StepHandlerCtx<'_, A>) -> Result<StepRun, RuntimeError>
where
    A: SkillAdapter,
{
    let StepHandlerCtx {
        runtime,
        workflow_dir,
        workflow_name,
        step,
        attempt,
        inputs,
        child_act: _,
        provenance: _,
        admission_refs,
        host: _,
        authority,
        loaded_skill: _,
    } = request;
    let tool = step
        .catalog_tool()
        .ok_or_else(|| RuntimeError::StepFailed {
            step_id: step.id.clone(),
            reason: "tool step missing tool name".to_owned(),
        })?;
    let call = crate::tool_catalogs::ToolCall::new(
        runtime.options.run.run_id(),
        &step.id,
        attempt,
        runtime.options.run_authority.as_ref(),
    );
    // A catalog tool runs with the scopes it declares.
    let scopes = crate::tool_catalogs::dispatch::inspect_catalog_tool(
        tool,
        &runtime.options.env,
        workflow_dir,
        &runtime.options.effects,
    )
    .map(|inspected| inspected.scopes)
    .map_err(|error| RuntimeError::StepFailed {
        step_id: step.id.clone(),
        reason: format!("tool {tool} could not be resolved: {error}"),
    })?;
    let tool_request = crate::tool_catalogs::dispatch::ToolDispatchRequest {
        tool: Cow::Borrowed(tool),
        inputs: Cow::Owned(inputs),
        resolved_inputs: Cow::Owned(JsonObject::new()),
        scopes: &scopes,
        env: &runtime.options.env,
        skill_directory: workflow_dir,
        credential_delivery: &crate::credentials::CredentialDelivery::none(),
        local_artifacts: &runtime.local_artifacts,
        skill_name: tool,
        allow_explicit_manifest_path: true,
        effect_admission: authority.as_ref().map(StepAuthorityContext::admission),
        reach: crate::kernel::authority::step_reach(runtime.options.run_authority.as_ref(), step),
        call: Some(&call),
    };
    // Source the tool manifest's artifact contract so the wrapped packet the
    // dispatcher folds into the claim (e.g. `digest_result`) is
    // exposed at the OUTER step as `<step>.<packet>.data`.
    let tool_artifacts = crate::tool_catalogs::dispatch::resolve_tool_artifacts(
        &tool_request,
        &runtime.options.effects,
    )?;
    // A native tool that only reads shares the gate; anything else, a tool
    // runx runs as a process included, holds it alone.
    let _pass = runtime
        .options
        .effect_pass(crate::tool_catalogs::native::only_reads(tool));
    let output = crate::tool_catalogs::dispatch::dispatch_tool(
        tool_request,
        &runtime.options.effects,
        &runtime.options.clock.now(),
        Instant::now(),
    )?;
    seal_effect_step(
        EffectStepSeal {
            runtime,
            workflow_dir,
            workflow_name,
            step,
            attempt,
            admission_refs,
            authority: authority.as_ref(),
            skill: format!("tool:{tool}"),
            call: None,
        },
        output,
        tool_artifacts.as_ref(),
    )
}

/// Everything a step owned by an effect seals with.
struct EffectStepSeal<'a, A> {
    runtime: &'a Runtime<A>,
    workflow_dir: &'a Path,
    workflow_name: &'a str,
    step: &'a WorkflowStep,
    attempt: u32,
    admission_refs: Vec<Reference>,
    authority: Option<&'a StepAuthorityContext>,
    skill: String,
    /// The governed call an operation step made.
    call: Option<crate::receipts::SealedCall>,
}

/// Project, gate, seal, finalize and persist a step whose output an effect
/// produced: a tool step or an operation step.
fn seal_effect_step<A>(
    seal: EffectStepSeal<'_, A>,
    mut output: InvocationOutput,
    artifacts: Option<&SkillArtifactContract>,
) -> Result<StepRun, RuntimeError>
where
    A: SkillAdapter,
{
    let EffectStepSeal {
        runtime,
        workflow_dir,
        workflow_name,
        step,
        attempt,
        admission_refs,
        authority,
        skill,
        call,
    } = seal;
    let provisional_projection = build_step_output_projection(step, &output, None, artifacts)?;
    let provisional_claim = contract_output_claim(&provisional_projection);
    prepare_effect_output_before_gate(
        step,
        authority,
        provisional_claim,
        &mut output,
        &runtime.options.effects,
    )?;
    let projection = build_step_output_projection(step, &output, None, artifacts)?;
    if output.succeeded() {
        // The packets the step declares, or else its tool's own, are checked
        // against the artifacts it holds, as any step's are.
        output.observations.contract.packet_schemas =
            crate::packet_validation::verify_declared_packets(
                &projection.outputs,
                step.artifacts.as_ref().or(artifacts),
                workflow_dir,
            )?;
    }
    let ephemeral_contract = build_ephemeral_step_output(step, &output, None, artifacts);
    let effect_claim = contract_output_claim(&projection);
    let seal_authority = authority
        .map(|authority| authority.authority(&runtime.options.effects))
        .transpose()?
        .unwrap_or_default();
    let receipt = seal_executed_step(
        StepSeal {
            scope: runtime.scope(workflow_name),
            step_id: &step.id,
            attempt,
            output: &output,
            claim: effect_claim,
            created_at: &runtime.options.clock.now(),
            authority: seal_authority,
            call,
            admission_refs,
            child_receipts: &[],
            descendant_receipts: &[],
            closure: None,
        },
        runtime.options.signature_policy(),
    )?;
    let mut settlement = None;
    for (persist, context) in [
        (false, "finalizing a sealed effect"),
        (true, "persisting a sealed effect"),
    ] {
        let request = EffectReceiptContext {
            step,
            workflow_dir,
            authority,
            claim: effect_claim,
            output: &mut output,
            receipt: &receipt,
            env: &runtime.options.env,
            signature_policy: runtime.options.signature_policy(),
            effects: &runtime.options.effects,
        };
        if persist {
            settlement = persist_effect_state_for_step(request)
                .map_err(|source| RuntimeError::engine(context, source))?;
        } else {
            finalize_effect_output_before_success(request)
                .map_err(|source| RuntimeError::engine(context, source))?;
        }
    }
    let admission_witness = StepAdmissionWitness::local_runtime(&step.id, receipt.id.as_str());
    Ok(StepRun {
        step_id: step.id.clone(),
        attempt,
        skill,
        method: step_runner(step),
        contract: projection.outputs,
        artifact_outputs: projection.artifact_outputs,
        ephemeral_contract: EphemeralValue::from_value(JsonValue::Object(ephemeral_contract)),
        outcome: output.into(),
        receipt,
        nested_receipts: Vec::new(),
        refusal: None,
        admission_witness,
        settlement,
    })
}

/// The step's runtime, as the kernel's runner for `process` implementations:
/// the installed copy of the tool the connector ships, with the operation's
/// credential. It keeps what each run observed: where the process ran and
/// the credential it received, whatever the run returned.
struct StepTools<'a, A> {
    runtime: &'a Runtime<A>,
    workflow_dir: &'a Path,
    /// Where the step's tools may go.
    reach: runx_contracts::Reach,
    observed: std::cell::RefCell<crate::observations::Observations>,
}

impl<A> crate::kernel::operation::ToolRunner for StepTools<'_, A>
where
    A: SkillAdapter,
{
    fn run(
        &self,
        tool: &str,
        manifest: &Path,
        inputs: JsonObject,
        scopes: &[String],
        delivery: &crate::credentials::CredentialDelivery,
    ) -> Result<runx_contracts::JsonValue, crate::kernel::operation::confirm::Unperformed> {
        let directory = manifest.parent().unwrap_or(self.workflow_dir);
        let output = crate::tool_catalogs::dispatch::dispatch_connector_tool(
            manifest,
            &crate::tool_catalogs::dispatch::ToolDispatchRequest {
                tool: Cow::Borrowed(tool),
                inputs: Cow::Owned(inputs),
                resolved_inputs: Cow::Owned(JsonObject::new()),
                scopes,
                env: &self.runtime.options.env,
                skill_directory: directory,
                credential_delivery: delivery,
                local_artifacts: &self.runtime.local_artifacts,
                skill_name: tool,
                allow_explicit_manifest_path: false,
                effect_admission: None,
                reach: self.reach,
                call: None,
            },
            Instant::now(),
        )?;
        {
            let mut observed = self.observed.borrow_mut();
            if let Some(boundary) = output.observations.boundary() {
                observed.boundary = Some(boundary);
            }
            for credential in output.observations.credentials() {
                observed.record_credential(credential).map_err(|error| {
                    crate::kernel::operation::confirm::Unperformed::MaybeSent {
                        code: "observation_failed",
                        reason: error.to_string(),
                    }
                })?;
            }
        }
        crate::tool_catalogs::dispatch::connector_tool_result(output)
    }
}

/// An operation step: the kernel executes the call its admission prepared,
/// and the step seals the `runx.provider.operation.v1` packet.
fn run_operation_step<A>(request: StepHandlerCtx<'_, A>) -> Result<StepRun, RuntimeError>
where
    A: SkillAdapter,
{
    let StepHandlerCtx {
        runtime,
        workflow_dir,
        workflow_name,
        step,
        attempt,
        admission_refs,
        authority,
        ..
    } = request;
    let authority = authority.ok_or_else(|| RuntimeError::StepFailed {
        step_id: step.id.clone(),
        reason: "an operation step runs only after the kernel admits it".to_owned(),
    })?;
    let context = authority
        .admission()
        .context::<crate::kernel::operation::OperationAdmission>()
        .ok_or_else(|| RuntimeError::StepFailed {
            step_id: step.id.clone(),
            reason: "the step's admission is not an operation admission".to_owned(),
        })?;
    let tools = StepTools {
        runtime,
        workflow_dir,
        reach: crate::kernel::authority::step_reach(runtime.options.run_authority.as_ref(), step),
        observed: Default::default(),
    };
    let started = Instant::now();
    let mut dispatched = None;
    let _pass = runtime.options.effect_pass(!context.mutation());
    let outcome = crate::kernel::operation::execute(
        context,
        &step.id,
        &runtime.options.env,
        workflow_dir,
        &tools,
        &mut dispatched,
    );
    // What the invocation was handed holds for a failed call as for one that
    // succeeded; a call refused before invocation observed neither.
    let mut observations = tools.observed.take();
    if let Some(dispatched) = dispatched {
        observations.boundary = Some(dispatched.boundary);
        if let Some(credential) = &dispatched.credential {
            observations.record_credential(credential)?;
        }
    }
    let mut effects = Vec::new();
    let output = match outcome {
        Ok(outcome) => {
            effects = outcome.effects;
            let mut output = crate::adapter_pipeline::AdapterProjection::from_started(started)
                .runtime_output(
                    crate::adapter::InvocationStatus::Success,
                    outcome.value,
                    None,
                    observations,
                );
            if let Some(ephemeral) = outcome.ephemeral {
                output.set_ephemeral(ephemeral);
            }
            output
        }
        Err(
            error @ (RuntimeError::ProviderEffectUnknown { .. }
            | RuntimeError::ProviderReadbackPending { .. }
            | RuntimeError::EffectState { .. }),
        ) => return Err(error),
        Err(error) => crate::adapter_pipeline::AdapterProjection::from_started(started)
            .failure(error.to_string(), observations),
    };
    let artifacts = crate::kernel::operation::packet_artifacts();
    let skill = context.skill_label();
    let call = context.sealed_call(
        effects,
        crate::execution::prepared_skill::prepared_context_ref(&runtime.options.env),
    )?;
    seal_effect_step(
        EffectStepSeal {
            runtime,
            workflow_dir,
            workflow_name,
            step,
            attempt,
            admission_refs,
            authority: Some(&authority),
            skill,
            call: Some(call),
        },
        output,
        Some(&artifacts),
    )
}

fn agent_task_output(
    response: ResolutionResponse,
    outcome: &RunOutcome,
) -> Result<InvocationOutput, RuntimeError> {
    let succeeded = *outcome == RunOutcome::Completed;
    // A failure says why, in the closure's reason, and names a requirement
    // its agent harness couldn't meet.
    let closure = response
        .payload
        .as_object()
        .and_then(|answer| answer.get("closure"))
        .and_then(JsonValue::as_object);
    let stated = closure
        .and_then(|closure| closure.get("reason"))
        .and_then(JsonValue::as_str)
        .filter(|reason| !reason.trim().is_empty());
    let unsupported = closure
        .and_then(|closure| closure.get("unsupported_requirement"))
        .and_then(JsonValue::as_str);
    let reason = match (unsupported, stated) {
        (Some(requirement), Some(stated)) => {
            Some(format!("unsupported_requirement {requirement}: {stated}"))
        }
        (Some(requirement), None) => Some(format!("unsupported_requirement {requirement}")),
        (None, stated) => stated.map(str::to_owned),
    };
    Ok(if succeeded {
        InvocationOutput::runtime_success(response.payload, 0)
    } else {
        let message = match reason {
            Some(reason) => format!("agent act ended {}: {reason}", outcome.label()),
            None => format!("agent act ended {}", outcome.label()),
        };
        InvocationOutput::runtime_failure(response.payload, message, 0)
    })
}

fn resolution_event_data(
    step: &WorkflowStep,
    request: &ResolutionRequest,
) -> Result<JsonValue, RuntimeError> {
    let request_value = serde_json::to_value(request)
        .and_then(serde_json::from_value)
        .map_err(|source| RuntimeError::json("serializing agent-task request", source))?;
    let mut data = JsonObject::new();
    data.insert("step_id".to_owned(), JsonValue::String(step.id.clone()));
    data.insert("request".to_owned(), request_value);
    Ok(JsonValue::Object(data))
}

fn agent_answer_outcome_value(
    step: &WorkflowStep,
    answer: &JsonValue,
) -> Result<RunOutcome, RuntimeError> {
    let outcome =
        agent_answer_outcome_or_completed(answer).map_err(|error| RuntimeError::StepFailed {
            step_id: step.id.clone(),
            reason: format!("{error}"),
        })?;
    if let StepKind::Agent(task) = &step.kind {
        crate::execution::outcome::check_turn_requirements(&task.requires, answer, outcome)
            .map_err(|reason| RuntimeError::StepFailed {
                step_id: step.id.clone(),
                reason,
            })?;
    }
    Ok(outcome)
}

// Function rationale: an approval step resolves, projects,
// and seals one trust-boundary decision without exposing a partially built receipt.
fn run_approval_step<A>(
    request: StepHandlerCtx<'_, A>,
    ask: &AskGate,
) -> Result<StepRun, RuntimeError>
where
    A: SkillAdapter,
{
    let StepHandlerCtx {
        runtime,
        workflow_dir,
        workflow_name,
        step,
        attempt,
        inputs,
        host,
        ..
    } = request;
    if ask.gate_type.as_deref() == Some(QUESTIONS_GATE) {
        return run_question_step(runtime, workflow_name, step, ask, attempt, &inputs, host);
    }
    let mut gate = approval_gate(ask, &inputs);
    // What the person approves is the reason they are asked.
    crate::approval::carry_payload(
        &mut gate,
        JsonValue::Object(JsonObject::from([(
            "reason".to_owned(),
            JsonValue::String(ask.reason.clone()),
        )])),
        &crate::paths::RunxPaths::resolve(&runtime.options.env, workflow_dir).artifacts(),
        runtime.options.run.run_id(),
    )
    .map_err(|message| RuntimeError::effect_state("carrying an ask", message))?;
    // The request is the gate at its address, not the step: a caller's
    // approval names the gate.
    let request_id = runtime.options.run.address(gate.id.as_str());
    let resolution = completed_approval_resolution(
        step,
        &gate,
        &request_id,
        resolve_step_approval(step, host, request_id.clone(), gate.clone())?,
    )?;
    let outputs = approval_outputs(step, &gate, &resolution)?;
    let output = InvocationOutput::runtime_success(JsonValue::Object(outputs.clone()), 0);
    let projection = project_step_claim(outputs, step.artifacts.as_ref());
    let receipt = seal_executed_step(
        StepSeal {
            scope: runtime.scope(workflow_name),
            step_id: &step.id,
            attempt,
            output: &output,
            claim: &projection.outputs,
            created_at: &runtime.options.clock.now(),
            authority: crate::receipts::SealAuthority::default(),
            call: None,
            admission_refs: Vec::new(),
            child_receipts: &[],
            descendant_receipts: &[],
            closure: None,
        },
        runtime.options.signature_policy(),
    )?;
    let admission_witness = StepAdmissionWitness::local_runtime(&step.id, receipt.id.as_str());
    Ok(StepRun {
        step_id: step.id.clone(),
        attempt,
        skill: "ask".to_owned(),
        method: step_runner(step),
        contract: projection.outputs,
        artifact_outputs: projection.artifact_outputs,
        ephemeral_contract: EphemeralValue::default(),
        outcome: output.into(),
        receipt,
        nested_receipts: Vec::new(),
        refusal: None,
        admission_witness,
        settlement: None,
    })
}

/// The gate type of an ask that puts questions to the person.
pub(super) const QUESTIONS_GATE: &str = "questions";

/// The questions an ask carries, as the agent states them: one to three,
/// each `{id, prompt, options?: [{label, detail?}], free_text?, required?}`
/// with at most four options, the recommended first, and free text where it
/// offers none.
pub(super) fn ask_questions(value: &JsonValue) -> Result<Vec<runx_contracts::Question>, String> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Asked {
        id: String,
        prompt: String,
        #[serde(default)]
        options: Vec<runx_contracts::QuestionOption>,
        #[serde(default)]
        free_text: bool,
        #[serde(default = "required_by_default")]
        required: bool,
    }
    fn required_by_default() -> bool {
        true
    }
    let asked: Vec<Asked> = value
        .clone()
        .deserialize_into()
        .map_err(|error| format!("questions are invalid: {error}"))?;
    if asked.is_empty() || asked.len() > 3 {
        return Err("an ask puts one to three questions".to_owned());
    }
    let mut ids = std::collections::BTreeSet::new();
    asked
        .into_iter()
        .map(|question| {
            let id = runx_contracts::schema::NonEmptyString::new(question.id.clone())
                .filter(|_| ids.insert(question.id.clone()))
                .ok_or_else(|| format!("question ids are distinct and named: {:?}", question.id))?;
            let prompt =
                runx_contracts::schema::NonEmptyString::new(question.prompt.trim().to_owned())
                    .ok_or_else(|| format!("question {id} has a prompt"))?;
            if question.options.len() > 4 {
                return Err(format!("question {id} offers at most four options"));
            }
            let mut labels = std::collections::BTreeSet::new();
            if !question
                .options
                .iter()
                .all(|option| labels.insert(option.label.as_str().to_owned()))
            {
                return Err(format!("question {id}'s options are distinct"));
            }
            let free_text = question.free_text || question.options.is_empty();
            Ok(runx_contracts::Question {
                question_type: if question.options.is_empty() {
                    "text".into()
                } else {
                    "choice".into()
                },
                id,
                prompt,
                description: None,
                required: question.required,
                options: question.options,
                free_text,
            })
        })
        .collect()
}

/// An ask that puts questions to the person: an input request carrying them,
/// answered in the person's lane and checked against them.
fn run_question_step<A>(
    runtime: &Runtime<A>,
    workflow_name: &str,
    step: &WorkflowStep,
    ask: &AskGate,
    attempt: u32,
    inputs: &JsonObject,
    host: &mut dyn Host,
) -> Result<StepRun, RuntimeError>
where
    A: SkillAdapter,
{
    let invalid = |reason: String| RuntimeError::StepFailed {
        step_id: step.id.clone(),
        reason,
    };
    let questions = ask_questions(
        inputs
            .get("questions")
            .ok_or_else(|| invalid("an ask of questions carries them".to_owned()))?,
    )
    .map_err(invalid)?;
    let request = ResolutionRequest::Input {
        id: runtime.options.run.address(&ask.gate).into(),
        questions: questions.clone(),
    };
    let Some(response) = host.resolve(request.clone())? else {
        return Err(RuntimeError::ResolutionPending {
            step_id: step.id.clone(),
            reason: format!("questions {} wait on the person", ask.gate),
            request: Box::new(request),
            child: None,
        });
    };
    if response.actor != runx_contracts::ResolutionResponseActor::Human {
        return Err(RuntimeError::ContinuationRejected {
            reason: format!("questions {} are the person's to answer", ask.gate),
        });
    }
    runx_contracts::check_answer(&questions, &response.payload).map_err(|reason| {
        RuntimeError::ContinuationRejected {
            reason: format!("the answer to {} was refused: {reason}", ask.gate),
        }
    })?;
    let outputs = JsonObject::from([("answers".to_owned(), response.payload)]);
    let output = InvocationOutput::runtime_success(JsonValue::Object(outputs.clone()), 0);
    let projection = project_step_claim(outputs, step.artifacts.as_ref());
    let receipt = seal_executed_step(
        StepSeal {
            scope: runtime.scope(workflow_name),
            step_id: &step.id,
            attempt,
            output: &output,
            claim: &projection.outputs,
            created_at: &runtime.options.clock.now(),
            authority: crate::receipts::SealAuthority::default(),
            call: None,
            admission_refs: Vec::new(),
            child_receipts: &[],
            descendant_receipts: &[],
            closure: None,
        },
        runtime.options.signature_policy(),
    )?;
    let admission_witness = StepAdmissionWitness::local_runtime(&step.id, receipt.id.as_str());
    Ok(StepRun {
        step_id: step.id.clone(),
        attempt,
        skill: "ask".to_owned(),
        method: step_runner(step),
        contract: projection.outputs,
        artifact_outputs: projection.artifact_outputs,
        ephemeral_contract: EphemeralValue::default(),
        outcome: output.into(),
        receipt,
        nested_receipts: Vec::new(),
        refusal: None,
        admission_witness,
        settlement: None,
    })
}

fn completed_approval_resolution(
    step: &WorkflowStep,
    gate: &ApprovalGate,
    request_id: &str,
    resolution: ApprovalResolution,
) -> Result<ApprovalResolution, RuntimeError> {
    if matches!(resolution, ApprovalResolution::Pending { .. }) {
        return Err(RuntimeError::ResolutionPending {
            step_id: step.id.clone(),
            reason: format!("approval gate '{}' is pending", gate.id),
            request: Box::new(ResolutionRequest::Approval {
                id: request_id.into(),
                gate: Box::new(gate.clone()),
            }),
            child: None,
        });
    }
    Ok(resolution)
}

/// The gate an ask step presents: its declared gate and reason, with the
/// step's inputs as the summary shown beside them.
pub(super) fn approval_gate(ask: &AskGate, inputs: &JsonObject) -> ApprovalGate {
    ApprovalGate {
        id: ask.gate.clone().into(),
        reason: ask.reason.clone().into(),
        gate_type: ask.gate_type.clone(),
        summary: (!inputs.is_empty()).then(|| inputs.clone()),
        payload: None,
        payload_artifact: None,
    }
}

pub(super) fn approval_outputs(
    step: &WorkflowStep,
    gate: &ApprovalGate,
    resolution: &ApprovalResolution,
) -> Result<JsonObject, RuntimeError> {
    let mut data = JsonObject::new();
    data.insert("approved".to_owned(), approved_value(resolution));
    data.insert(
        "gate_id".to_owned(),
        JsonValue::String(gate.id.as_str().to_owned()),
    );
    if let Some(gate_type) = &gate.gate_type {
        data.insert("gate_type".to_owned(), JsonValue::String(gate_type.clone()));
    }
    data.insert(
        "idempotency_key".to_owned(),
        JsonValue::String(resolution.idempotency_key().to_owned()),
    );
    data.insert(
        "status".to_owned(),
        JsonValue::String(approval_status(resolution).to_owned()),
    );
    if let Some(actor) = resolution.actor() {
        data.insert("actor".to_owned(), JsonValue::String(actor_name(actor)));
    }
    if let Some(reason) = resolution.reason() {
        data.insert("reason".to_owned(), JsonValue::String(reason.to_owned()));
    }

    // The canonical artifact envelope: the packet it declares, and its data.
    let mut packet = JsonObject::new();
    if let Some(packet_id) = artifact_packet(step)? {
        packet.insert(
            "schema".to_owned(),
            JsonValue::String(packet_id.into_string()),
        );
    }
    packet.insert("data".to_owned(), JsonValue::Object(data));

    let mut outputs = JsonObject::new();
    outputs.insert(
        artifact_wrap_as(step)?.to_owned(),
        JsonValue::Object(packet),
    );
    Ok(outputs)
}

pub(super) fn approved_value(resolution: &ApprovalResolution) -> JsonValue {
    resolution
        .approved()
        .map_or(JsonValue::Null, JsonValue::Bool)
}

pub(super) fn approval_status(resolution: &ApprovalResolution) -> &'static str {
    match resolution {
        ApprovalResolution::Approved { .. } => "approved",
        ApprovalResolution::Denied { .. } => "denied",
        ApprovalResolution::Pending { .. } => "pending",
    }
}

pub(super) fn actor_name(actor: &ResolutionResponseActor) -> String {
    match actor {
        ResolutionResponseActor::Human => "human".to_owned(),
        ResolutionResponseActor::Agent => "agent".to_owned(),
    }
}

pub(super) fn artifact_wrap_as(step: &WorkflowStep) -> Result<&str, RuntimeError> {
    let Some(artifacts) = &step.artifacts else {
        return Ok("approval");
    };
    Ok(artifacts.wrap_as.as_deref().unwrap_or("approval"))
}

pub(super) fn artifact_packet(
    step: &WorkflowStep,
) -> Result<Option<runx_contracts::PacketId>, RuntimeError> {
    Ok(step
        .artifacts
        .as_ref()
        .and_then(|artifacts| artifacts.packet.clone()))
}

pub(super) fn runtime_error_step_run<A>(
    runtime: &Runtime<A>,
    workflow_name: &str,
    step: &WorkflowStep,
    attempt: u32,
    error: RuntimeError,
) -> Result<StepRun, RuntimeError>
where
    A: SkillAdapter,
{
    // A managed agent that stopped seals why it stopped.
    let closure = match &error {
        #[cfg(feature = "agent")]
        RuntimeError::ManagedAgentResolution { source, .. } => Some(StepSealEnding {
            outcome: RunOutcome::Failed,
            reason: Some(source.reason()),
        }),
        _ => None,
    };
    let output = InvocationOutput::runtime_failure(
        JsonValue::Object(error.public_failure_projection()),
        error.to_string(),
        0,
    );
    // An agent step that fails still names the calls its turn completed.
    let calls = match &step.kind {
        StepKind::Agent(_) => runtime.options.run.calls(&step.id, attempt)?,
        _ => Vec::new(),
    };
    let lineage = StepReceiptLineage::of_turn(
        &calls
            .iter()
            .filter_map(|call| match &call.outcome {
                crate::execution::run_record::CallOutcome::Completed(run) => Some(run.as_ref()),
                _ => None,
            })
            .collect::<Vec<_>>(),
    );
    let projection = project_step_claim(JsonObject::new(), None);
    let receipt = seal_step(
        StepSeal {
            scope: runtime.scope(workflow_name),
            step_id: &step.id,
            attempt,
            output: &output,
            claim: &projection.outputs,
            created_at: &runtime.options.clock.now(),
            authority: crate::receipts::SealAuthority::default(),
            call: None,
            admission_refs: Vec::new(),
            child_receipts: &lineage.direct_children,
            descendant_receipts: &lineage.descendants,
            closure,
        },
        runtime.options.signature_policy(),
    )?;
    let admission_witness = StepAdmissionWitness::local_runtime(&step.id, receipt.id.as_str());
    Ok(StepRun {
        step_id: step.id.clone(),
        attempt,
        skill: step
            .skill()
            .map_or(step.id.as_str(), |target| target.skill.as_str())
            .to_owned(),
        method: step_runner(step),
        contract: projection.outputs,
        artifact_outputs: projection.artifact_outputs,
        ephemeral_contract: EphemeralValue::default(),
        outcome: output.into(),
        receipt,
        nested_receipts: lineage.into_nested_receipts(),
        refusal: None,
        admission_witness,
        settlement: None,
    })
}

fn seal_executed_step(
    request: StepSeal<'_>,
    signature_policy: RuntimeReceiptSignaturePolicy<'_>,
) -> Result<Receipt, RuntimeError> {
    seal_step(request, signature_policy)
        .map_err(|source| RuntimeError::engine("sealing a workflow step receipt", source))
}

fn step_admission_witness(
    step_id: &str,
    receipt_id: &str,
    authority: Option<&super::admission::StepAuthorityContext>,
) -> StepAdmissionWitness {
    authority.map_or_else(
        || StepAdmissionWitness::local_runtime(step_id, receipt_id),
        |authority| {
            StepAdmissionWitness::with_authority(
                step_id,
                receipt_id,
                authority.admission_witness().clone(),
            )
        },
    )
}

#[cfg(test)]
mod step_requirements {
    use runx_parser::{StepKind, ToolTarget};

    use crate::adapter::Executable;

    #[test]
    fn a_worker_step_projects_its_environment_without_interpretation()
    -> Result<(), Box<dyn std::error::Error>> {
        let graph = crate::execution::graph::testing::method_workflow(
            "worker-requirements",
            r#"
minimum_runx: 0.12.0
methods:
  run:
    steps:
      - id: compute
        tool:
          module: compute.mjs
          environment:
            required: [REGION]
            optional: [TRACE_LABEL]
"#,
        )?;
        let step = graph.steps.first().ok_or("missing step")?;
        let StepKind::Tool(ToolTarget::Worker(worker)) = &step.kind else {
            return Err("expected a worker step".into());
        };

        let requirements = super::step_requirements(&Executable::Worker(worker.clone()));

        assert_eq!(requirements.environment.required, ["REGION"]);
        assert_eq!(requirements.environment.optional, ["TRACE_LABEL"]);
        Ok(())
    }
}
