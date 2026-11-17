//! A write's claim (the loop contract's E7): one event appended to the
//! calling package's own store at the version the caller decided at, made
//! after the person's approval and just before the act is attempted. A key
//! that moved refuses the act before anything is sent, so a decision made on
//! a record that changed since is never carried out.
//!
//! The claim is the package's own `store/events.append`, authorized as every
//! store call is, so its stream is the package's own, placed by the run's
//! scope. It's appended under a key derived from the act's, so recovering an
//! act whose claim landed replays the claim rather than conflicting with it.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use runx_contracts::connector::{Implementation, OperationAccess, OperationName};
use runx_contracts::{ConnectorInstance, JsonValue};

use super::call::OperationCall;
use super::confirm::{self, Checks, Recovery};
use super::credentials;
use super::{AdmittedCall, Grounds, OperationEffect, Sealed, invalid, provider_key, refused};
use crate::effects::{EffectStepRequest, RuntimeEffectError};
use crate::kernel::admission;
use crate::kernel::connectors::LoadedConnector;

/// A claim authorized for one write, carried from admission to the attempt.
#[derive(Clone)]
pub(crate) struct Claim {
    connector: Arc<LoadedConnector>,
    operation: OperationName,
    instance: ConnectorInstance,
    /// The stream as the store places it: the package's own.
    target: String,
    /// The key claimed, and the version it was decided at.
    key: String,
    expected: u64,
    arguments: JsonValue,
    idempotency_key: Option<String>,
}

/// Why a claim stopped its write.
pub(crate) enum Refusal {
    /// The key moved since the caller read it, or its key holds another
    /// claim: the write's decision no longer stands.
    Moved(String),
    /// The store couldn't take the claim; nothing was sent.
    Failed(String),
}

impl OperationEffect {
    /// The claim `call` makes, authorized as its package's own store append
    /// under the run's chain and authority, or none when it makes none. Only a
    /// write claims, and only from a step that names the store's append.
    pub(super) fn claim(
        &self,
        request: EffectStepRequest<'_>,
        target: &runx_parser::OperationStepTarget,
        call: &OperationCall,
    ) -> Result<Option<Claim>, RuntimeEffectError> {
        let Some(claim) = &call.claim else {
            return Ok(None);
        };
        let declared = target.claims.clone().ok_or_else(|| {
            invalid(format!(
                "a claim is the package's own {}/{}, which the step doesn't name",
                runx_parser::CLAIM_REQUIREMENT,
                runx_parser::CLAIM_OPERATION
            ))
        })?;
        let key = call
            .idempotency_key
            .as_deref()
            .ok_or_else(|| invalid("a claim is made by a write, under the write's key"))?;
        let claim_target = runx_parser::OperationStepTarget {
            requirement: runx_parser::CLAIM_REQUIREMENT.to_owned(),
            operation: OperationName::new(runx_parser::CLAIM_OPERATION.to_owned()),
            from: Vec::new(),
            declared: Some(declared),
            loaded: None,
            claims: None,
        };
        let claim_call = OperationCall {
            inputs: claim.inputs(),
            idempotency_key: Some(format!("claim:{key}")),
            ..OperationCall::default()
        };
        let grounds = Grounds::of(request, &claim_target)?;
        let authorized =
            admission::authorize(&self.evaluation(request, &claim_target, &claim_call, &grounds))
                .map_err(refused)?;
        let operation = authorized.operation();
        if operation.access() != OperationAccess::Mutate
            || !matches!(operation.implementation, Some(Implementation::Store(_)))
        {
            return Err(invalid("a claim is an append to the package's own store"));
        }
        let idempotency_key = provider_key(
            operation,
            authorized.intent(),
            authorized.intent_digest().as_str(),
        );
        let (connector, operation, instance, target) = authorized.into_call();
        Ok(Some(Claim {
            connector,
            operation,
            instance,
            target,
            key: claim.key.clone(),
            expected: claim.expected_version,
            arguments: JsonValue::Object(claim_call.inputs),
            idempotency_key,
        }))
    }
}

impl Claim {
    /// Append the claim, written by `writer`, the package the write is made
    /// from. Committed, or replayed because this act's claim landed before,
    /// lets the write go on; anything else refuses it.
    pub(super) fn append(
        &self,
        env: &BTreeMap<String, String>,
        cwd: &Path,
        writer: Option<&crate::identity::ChainLink>,
    ) -> Result<(), Refusal> {
        let operation = &self.connector.manifest.operations[&self.operation];
        let Some(implementation) = operation.implementation.as_ref() else {
            return Err(Refusal::Failed(
                "the store implements no append here".to_owned(),
            ));
        };
        let secret = credentials::held_locally(env, cwd, &self.connector, &self.instance)
            .map_err(Refusal::Failed)?;
        let call = AdmittedCall {
            connector: self.connector.manifest.name.as_str(),
            operation: self.operation.as_str(),
            implementation: implementation.kind(),
            target: &self.target,
            arguments: &self.arguments,
            parameters: &self.instance.parameters,
            idempotency_key: self.idempotency_key.as_deref(),
            credential: secret.as_ref(),
            session_principal: None,
            env,
            held: Vec::new(),
            writer,
            withheld: &[],
            access: OperationAccess::Mutate,
            _sealed: Sealed,
        };
        let driver = crate::drivers::store::Store { call: &call };
        let checks = Checks {
            preconditions: Vec::new(),
            readbacks: vec![crate::drivers::store::readback_matches()],
        };
        let recovery = Recovery {
            acknowledged: None,
            resend_safe: true,
            may_charge: false,
        };
        let readback = confirm::run(&driver, &call, &checks, recovery, &mut |_| Ok(()))
            .map_err(|failure| Refusal::Failed(failure.reason()))?;
        let answer = readback.result.as_object();
        let status = answer
            .and_then(|answer| answer.get("status"))
            .and_then(JsonValue::as_str);
        match status {
            Some("committed" | "replayed") => Ok(()),
            _ => Err(Refusal::Moved(format!(
                "the claim found {} in {} {}, not at version {}, which the write was decided at, so nothing was sent",
                self.key,
                self.target,
                match (
                    status,
                    answer
                        .and_then(|answer| answer.get("version"))
                        .and_then(JsonValue::as_u64)
                ) {
                    (Some("version_conflict"), Some(version)) => format!("at version {version}"),
                    (Some(status), _) => format!("answering {status}"),
                    (None, _) => "answering nothing".to_owned(),
                },
                self.expected
            ))),
        }
    }
}
