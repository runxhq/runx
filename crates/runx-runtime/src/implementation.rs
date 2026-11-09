//! What a run is started by: the digest of the sources each binary was built
//! from (reuse contract, `implementation`). The runtime's is made by its
//! build script. The worker's is what the worker gives in its handshake, and
//! a run has one only when its own method can run JavaScript: such a run
//! starts its worker as it starts, so a runtime built beside the worker's
//! sources refuses a worker that reports others before the run begins. A run
//! records its implementation when it starts, and a resume under another is
//! refused, so a build of changed sources never replays old history; a run
//! that runs no JavaScript is never tied to the worker's sources.

use std::collections::BTreeMap;

use runx_contracts::RunImplementation;
use runx_contracts::schema::NonEmptyString;
use runx_parser::{StepKind, ToolTarget, Workflow};

use crate::RuntimeError;
use crate::adapters::javascript::JavaScriptAdapter;

/// Whether a run of `workflow` can run JavaScript itself: a script step, or
/// a tool step on the worker. Nothing else reaches the worker. A catalog
/// tool, by a step or by a call a step declares, is native or a manifest's
/// process; an operation is its connector's; a skill step's child decides
/// from its own method.
pub(crate) fn runs_javascript(workflow: &Workflow) -> bool {
    workflow.steps.iter().any(|step| {
        matches!(
            step.kind,
            StepKind::Script(_) | StepKind::Tool(ToolTarget::Worker(_))
        )
    })
}

/// How a run starts: the implementation it records, and the refusal it
/// meets before any step runs, if any.
pub(crate) struct Start {
    pub(crate) implementation: RunImplementation,
    pub(crate) refusal: Option<RuntimeError>,
}

/// How a run of `workflow` from `workflow_dir` starts on `runtime`: its own
/// scripts' declared operations admitted first, as each call of them would
/// be, then its worker started if it runs JavaScript. A run one of whose
/// scripts can't be admitted is refused as it starts: it runs no step and no
/// JavaScript, starts no worker and records none.
pub(crate) fn start<A>(
    runtime: &crate::Runtime<A>,
    workflow_dir: &std::path::Path,
    workflow: &Workflow,
) -> Result<Start, RuntimeError>
where
    A: crate::SkillAdapter,
{
    if let Some(refusal) =
        crate::execution::runner::declared_operations_refusal(runtime, workflow_dir, workflow)?
    {
        return Ok(Start {
            implementation: runtime_alone(),
            refusal: Some(refusal),
        });
    }
    Ok(Start {
        implementation: of_run(workflow, runtime.javascript(), &runtime.options().env)?,
        refusal: None,
    })
}

/// The implementation a run of `workflow` under `env` runs on: this runtime,
/// and the worker it runs its JavaScript on, if it runs any.
fn of_run(
    workflow: &Workflow,
    javascript: &JavaScriptAdapter,
    env: &BTreeMap<String, String>,
) -> Result<RunImplementation, RuntimeError> {
    Ok(implementation(
        worker(workflow, javascript, env)?.map(Into::into),
    ))
}

/// The digest of the worker a run of `workflow` runs its JavaScript on,
/// started now on `javascript`'s session, which keeps it for the run's first
/// call to it; none, and no worker started, for a run that runs none.
fn worker(
    workflow: &Workflow,
    javascript: &JavaScriptAdapter,
    env: &BTreeMap<String, String>,
) -> Result<Option<String>, RuntimeError> {
    if !runs_javascript(workflow) {
        return Ok(None);
    }
    javascript.worker_digest(env).map(Some)
}

/// This runtime alone: the implementation of a run that runs no JavaScript
/// now.
pub(crate) fn runtime_alone() -> RunImplementation {
    implementation(None)
}

fn implementation(worker: Option<NonEmptyString>) -> RunImplementation {
    RunImplementation {
        runtime: env!("RUNX_RUNTIME_SOURCE_DIGEST").into(),
        worker,
    }
}

/// The digest of the worker's sources this runtime was built beside; none
/// for a build without them to hand.
pub(crate) fn expected_worker() -> Option<&'static str> {
    option_env!("RUNX_JS_WORKER_SOURCE_DIGEST")
}

/// Why a run started under `started` can't go on under `current`: a run
/// resumes only under the implementation that started it.
pub(crate) fn refusal(
    started: Option<&RunImplementation>,
    current: &RunImplementation,
) -> Option<String> {
    let describe = |implementation: &RunImplementation| match &implementation.worker {
        Some(worker) => format!("runtime {}, worker {worker}", implementation.runtime),
        None => format!("runtime {}, no worker", implementation.runtime),
    };
    match started {
        Some(started) if started == current => None,
        Some(started) => Some(format!(
            "the run was started by {} and this runx is {}; a run resumes only under the implementation that started it",
            describe(started),
            describe(current)
        )),
        None => Some(format!(
            "the run was started before runx recorded its implementation; finish it with the runx that started it (this runx is {})",
            describe(current)
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use runx_contracts::RunImplementation;

    use super::{implementation, refusal};

    mod digest {
        include!("../source_digest.rs");
    }

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn write(path: &Path, text: &str) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, text)
    }

    /// The digest reads the crate and the workspace crates it depends on, all
    /// but what no binary is built from, so the same tree gives the same
    /// digest twice, a change to a dependency's source another, and a change
    /// to a test, or to a crate it doesn't depend on, none.
    #[test]
    fn the_same_sources_give_the_same_digest_and_a_changed_one_another() -> TestResult {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        write(&root.join("Cargo.toml"), "[workspace]\n")?;
        write(
            &root.join("runx-app/Cargo.toml"),
            "[package]\nname = \"runx-app\"\n\n[dependencies]\nrunx-lib.workspace = true\nserde = \"1\"\n\n[dev-dependencies]\nrunx-other.workspace = true\n",
        )?;
        write(&root.join("runx-app/src/main.rs"), "fn main() {}\n")?;
        write(&root.join("runx-app/assets/voice.md"), "speak\n")?;
        write(&root.join("runx-app/tests/it.rs"), "#[test] fn it() {}\n")?;
        write(
            &root.join("runx-lib/Cargo.toml"),
            "[package]\nname = \"runx-lib\"\n",
        )?;
        write(&root.join("runx-lib/src/lib.rs"), "pub fn one() {}\n")?;
        write(
            &root.join("runx-other/Cargo.toml"),
            "[package]\nname = \"runx-other\"\n",
        )?;
        write(&root.join("runx-other/src/lib.rs"), "pub fn two() {}\n")?;
        let app = root.join("runx-app");
        let (first, _) = digest::source_digest(&app)?;
        assert_eq!(
            digest::source_digest(&app)?.0,
            first,
            "the same tree, the same digest"
        );
        write(
            &root.join("runx-app/tests/it.rs"),
            "#[test] fn changed() {}\n",
        )?;
        write(&root.join("runx-other/src/lib.rs"), "pub fn three() {}\n")?;
        assert_eq!(
            digest::source_digest(&app)?.0,
            first,
            "a test, and a crate it doesn't build from, change nothing"
        );
        write(&root.join("runx-app/assets/voice.md"), "whisper\n")?;
        let embedded = digest::source_digest(&app)?.0;
        assert_ne!(embedded, first, "a file the crate embeds is its source");
        write(&root.join("runx-lib/src/lib.rs"), "pub fn changed() {}\n")?;
        assert_ne!(
            digest::source_digest(&app)?.0,
            embedded,
            "a dependency's source is the binary's"
        );
        Ok(())
    }

    #[test]
    fn a_resume_under_another_implementation_names_both() {
        let now = implementation(Some("sha256:worker".into()));
        assert_eq!(refusal(Some(&now), &now), None);
        let earlier = RunImplementation {
            runtime: "sha256:earlier".into(),
            worker: now.worker.clone(),
        };
        let refused = refusal(Some(&earlier), &now).unwrap_or_default();
        assert!(
            refused.contains("runtime sha256:earlier") && refused.contains(now.runtime.as_str()),
            "{refused}"
        );
        let older = refusal(None, &now).unwrap_or_default();
        assert!(
            older.contains("started before runx recorded its implementation; finish it with the runx that started it"),
            "a run that records none says what to do: {older}"
        );
    }
}
