//! The protocol every local driver runs under, and the readback check.
//!
//! A driver performs what it is handed and reports what it saw. The kernel
//! decides the rest, once for every implementation: a write with a recorded
//! acknowledgement is never performed again, an acknowledgement is recorded
//! in custody before anything is read back, every confirming read must match
//! the acknowledged write, a write that may have reached the provider is
//! never reported as refused, and a write's evidence is its confirming reads,
//! never its own answer. A write the provider cannot read back declares no
//! readbacks: its evidence is its recorded acknowledgement, which the receipt
//! seals as evidence of the effect, never as its finality.
//!
//! A repeated write is one approved call over an exact list of items. Each
//! item's outcome is recorded as it lands, a refused item does not stop the
//! others, a provider that refuses the caller (a rate limit) settles every
//! later item as unsent and defers the checking reads to the retry, an item
//! that may have landed stops the batch as an unknown outcome, and every
//! applied item is read back. A recovered batch resumes after its last
//! recorded item.

use runx_contracts::connector::{ReadbackExpected, ReadbackMatch};
use runx_contracts::{JsonObject, JsonValue, canonical_stable_json, sha256_prefixed};

use super::credentials::CredentialSecret;
use super::packet::Readback;
use super::{AdmittedCall, Failure};
use crate::kernel::custody::Acknowledgement;

/// What a driver saw performing a request: the decoded value and the digest
/// of each provider answer behind it.
pub(crate) struct Observed {
    pub value: JsonValue,
    pub evidence: Vec<String>,
}

/// Why a driver could not perform the call, as a fact about the provider.
#[derive(Debug)]
pub(crate) enum Unperformed {
    /// Nothing was sent.
    NotSent { code: &'static str, reason: String },
    /// The provider answered that it did not act, with the answer when it
    /// gave one.
    Refused {
        code: &'static str,
        reason: String,
        answer: Option<JsonValue>,
    },
    /// The provider refused the caller rather than the request: a rate
    /// limit, a credential it does not accept, or a billing hold. It did not
    /// act, and it will refuse anything more, until `retry_at` when it says.
    Halted {
        code: &'static str,
        reason: String,
        retry_at: Option<String>,
    },
    /// The request may have reached the provider, and nothing shows that it
    /// did not act.
    MaybeSent { code: &'static str, reason: String },
}

impl Unperformed {
    pub(crate) fn reason(self) -> String {
        match self {
            Self::NotSent { reason, .. }
            | Self::Refused { reason, .. }
            | Self::Halted { reason, .. }
            | Self::MaybeSent { reason, .. } => reason,
        }
    }
}

/// One of the reads the kernel checks around a call.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Check {
    /// The `n`th read that must hold before the call is performed.
    Precondition(usize),
    /// The `readback`th independent read confirming a write, for `item` of a
    /// repeated write.
    Readback {
        readback: usize,
        item: Option<usize>,
    },
}

/// What each checking read must match, from the implementation's manifest.
#[derive(Default)]
pub(crate) struct Checks {
    pub preconditions: Vec<Vec<ReadbackMatch>>,
    pub readbacks: Vec<Vec<ReadbackMatch>>,
}

/// What custody knows of an earlier attempt at this call.
pub(crate) struct Recovery {
    pub acknowledged: Option<Acknowledgement>,
    /// Whether a request that may have landed can be sent again.
    pub resend_safe: bool,
}

/// One local implementation, as the kernel drives it.
pub(crate) trait Driver {
    /// The items a repeated write is sent for, in order, or `None` when the
    /// call is one request.
    fn items(&self) -> Result<Option<Vec<JsonValue>>, String> {
        Ok(None)
    }

    /// The call, or `item` of a repeated write: a read's result or a write's
    /// acknowledgement.
    fn perform(&self, item: Option<usize>) -> Result<Observed, Unperformed>;

    /// One checking read. A readback sees the acknowledged `write`; a
    /// precondition sees null.
    fn check(&self, check: Check, write: &JsonValue) -> Result<Observed, String>;

    /// The operation's result and the provider's id for it, from the
    /// acknowledgement and what confirmed it.
    fn result(
        &self,
        write: &JsonValue,
        confirmed: &[JsonValue],
    ) -> Result<(JsonValue, Option<String>), String>;
}

/// A driver as the kernel holds it: nothing it answers repeats the credential
/// the call presented, a failure's reason and a refusal's answer included,
/// before the kernel records, returns or seals any of it.
struct Custody<'a> {
    driver: &'a dyn Driver,
    credential: Option<&'a CredentialSecret>,
}

impl Custody<'_> {
    fn text(&self, text: String) -> String {
        match self.credential {
            Some(secret) => secret.scrub_text(text),
            None => text,
        }
    }

    fn value(&self, value: JsonValue) -> JsonValue {
        match self.credential {
            Some(secret) => secret.scrub(value),
            None => value,
        }
    }

    fn observed(&self, observed: Observed) -> Observed {
        Observed {
            value: self.value(observed.value),
            evidence: observed.evidence,
        }
    }
}

impl Driver for Custody<'_> {
    fn items(&self) -> Result<Option<Vec<JsonValue>>, String> {
        self.driver.items().map_err(|reason| self.text(reason))
    }

    fn perform(&self, item: Option<usize>) -> Result<Observed, Unperformed> {
        self.driver
            .perform(item)
            .map(|observed| self.observed(observed))
            .map_err(|failure| match failure {
                Unperformed::NotSent { code, reason } => Unperformed::NotSent {
                    code,
                    reason: self.text(reason),
                },
                Unperformed::Refused {
                    code,
                    reason,
                    answer,
                } => Unperformed::Refused {
                    code,
                    reason: self.text(reason),
                    answer: answer.map(|answer| self.value(answer)),
                },
                Unperformed::Halted {
                    code,
                    reason,
                    retry_at,
                } => Unperformed::Halted {
                    code,
                    reason: self.text(reason),
                    retry_at,
                },
                Unperformed::MaybeSent { code, reason } => Unperformed::MaybeSent {
                    code,
                    reason: self.text(reason),
                },
            })
    }

    fn check(&self, check: Check, write: &JsonValue) -> Result<Observed, String> {
        self.driver
            .check(check, write)
            .map(|observed| self.observed(observed))
            .map_err(|reason| self.text(reason))
    }

    fn result(
        &self,
        write: &JsonValue,
        confirmed: &[JsonValue],
    ) -> Result<(JsonValue, Option<String>), String> {
        self.driver
            .result(write, confirmed)
            .map_err(|reason| self.text(reason))
    }
}

/// Run `driver` for the admitted `call` under `checks`.
pub(super) fn run(
    driver: &dyn Driver,
    call: &AdmittedCall<'_>,
    checks: &Checks,
    recovery: Recovery,
    record: &mut dyn FnMut(&Acknowledgement) -> Result<(), String>,
) -> Result<Readback, Failure> {
    let driver = &Custody {
        driver,
        credential: call.credential,
    };
    let items = match call.mutation() {
        true => driver
            .items()
            .map_err(|reason| rejected("invalid_request", reason))?,
        false => None,
    };
    let (write, confirmed, evidence) = match items {
        None => once(driver, call, checks, recovery, record)?,
        Some(items) => repeated(driver, call, checks, &items, recovery, record)?,
    };
    let (result, operation_id) = driver
        .result(&write, &confirmed)
        .map_err(|reason| match call.mutation() {
            true => Failure::ReadbackPending(reason),
            false => rejected("invalid_response", reason),
        })?;
    let readback_ref = if call.mutation() && checks.readbacks.is_empty() {
        // This write's acknowledgement: the act it answers and what it said.
        let acknowledgement = canonical_stable_json(&serde_json::json!({
            "operation": call.operation,
            "target": call.target,
            "idempotency_key": call.idempotency_key,
            "acknowledgement": &write,
        }))
        .map_err(|error| {
            Failure::ReadbackPending(format!("the acknowledgement cannot be digested: {error}"))
        })?;
        format!(
            "runx:{}-acknowledgement:{}",
            call.implementation,
            sha256_prefixed(acknowledgement.as_bytes())
        )
    } else {
        format!(
            "runx:{}-readback:{}",
            call.implementation,
            sha256_prefixed(evidence.join("\n").as_bytes())
        )
    };
    Ok(Readback {
        provider: call.connector.to_owned(),
        operation: call.operation.to_owned(),
        target: call.target.to_owned(),
        result,
        readback_ref,
        operation_id,
        idempotency_key: call.idempotency_key.map(str::to_owned),
    })
}

type Settled = (JsonValue, Vec<JsonValue>, Vec<String>);

fn once(
    driver: &dyn Driver,
    call: &AdmittedCall<'_>,
    checks: &Checks,
    recovery: Recovery,
    record: &mut dyn FnMut(&Acknowledgement) -> Result<(), String>,
) -> Result<Settled, Failure> {
    let mutation = call.mutation();
    let mut evidence = Vec::new();
    let write = match recovery.acknowledged {
        Some(acknowledgement) => acknowledgement.write_result,
        None => {
            preconditions(driver, call, checks)?;
            let performed = driver.perform(None).map_err(|failure| match failure {
                Unperformed::NotSent { code, reason }
                | Unperformed::Refused { code, reason, .. }
                | Unperformed::Halted { code, reason, .. } => rejected(code, reason),
                Unperformed::MaybeSent { reason, .. } if mutation => Failure::Unknown(reason),
                Unperformed::MaybeSent { code, reason } => Failure::Rejected {
                    status: 0,
                    code: code.to_owned(),
                    reason,
                    release: false,
                },
            })?;
            if mutation {
                record(&acknowledgement(performed.value.clone())).map_err(Failure::Unknown)?;
            } else {
                evidence = performed.evidence;
            }
            performed.value
        }
    };
    let mut confirmed = Vec::with_capacity(checks.readbacks.len());
    for (index, matches) in checks.readbacks.iter().enumerate() {
        let observed = driver
            .check(
                Check::Readback {
                    readback: index,
                    item: None,
                },
                &write,
            )
            .map_err(Failure::ReadbackPending)?;
        verify(matches, call.arguments, None, &write, &observed.value)
            .map_err(Failure::ReadbackPending)?;
        evidence.extend(observed.evidence);
        confirmed.push(observed.value);
    }
    Ok((write, confirmed, evidence))
}

/// A repeated write: every item's outcome, then a readback of every applied
/// item. The acknowledgement is the list of outcomes, recorded after each.
fn repeated(
    driver: &dyn Driver,
    call: &AdmittedCall<'_>,
    checks: &Checks,
    items: &[JsonValue],
    recovery: Recovery,
    record: &mut dyn FnMut(&Acknowledgement) -> Result<(), String>,
) -> Result<Settled, Failure> {
    let mut outcomes = match &recovery.acknowledged {
        Some(acknowledgement) => acknowledgement
            .write_result
            .as_array()
            .cloned()
            .ok_or_else(|| {
                Failure::Unknown("the recorded batch is not a list of outcomes".to_owned())
            })?,
        None => {
            preconditions(driver, call, checks)?;
            Vec::new()
        }
    };
    if outcomes.len() > items.len() {
        return Err(Failure::Unknown(
            "the recorded batch has more outcomes than items".to_owned(),
        ));
    }
    if recovery.acknowledged.is_some() && outcomes.len() < items.len() && !recovery.resend_safe {
        return Err(Failure::Unknown(format!(
            "item {} of {} may have reached the provider before the run stopped; reconcile it before resuming",
            outcomes.len() + 1,
            items.len()
        )));
    }
    let mut halt = None;
    for index in outcomes.len()..items.len() {
        let mut halted = None;
        let settled = match driver.perform(Some(index)) {
            Ok(observed) => outcome("applied", [("acknowledgement", Some(observed.value))]),
            Err(Unperformed::NotSent { code, reason }) => refused(code, reason, []),
            Err(Unperformed::Refused {
                code,
                reason,
                answer,
            }) => refused(code, reason, [("answer", answer)]),
            Err(Unperformed::Halted {
                code,
                reason,
                retry_at,
            }) => {
                halted = Some(code);
                refused(
                    code,
                    reason,
                    [("retry_at", retry_at.map(JsonValue::String))],
                )
            }
            Err(Unperformed::MaybeSent { reason, .. }) => {
                return Err(Failure::Unknown(format!(
                    "item {} of {}: {reason}",
                    index + 1,
                    items.len()
                )));
            }
        };
        outcomes.push(settled);
        // A provider that refuses the caller refuses every later item too,
        // so none is sent and the batch settles here.
        if let Some(code) = halted {
            let reason = format!("item {} of {} halted the batch", index + 1, items.len());
            outcomes.resize(
                items.len(),
                outcome(
                    "unsent",
                    [
                        ("code", Some(JsonValue::String(code.to_owned()))),
                        ("reason", Some(JsonValue::String(reason))),
                    ],
                ),
            );
        }
        record(&acknowledgement(JsonValue::Array(outcomes.clone()))).map_err(Failure::Unknown)?;
        if halted.is_some() {
            halt = halted;
            break;
        }
    }
    // A provider that just refused the caller would refuse the checking reads
    // too. The outcomes are recorded, so the reads wait for the retry, which
    // recovers them without sending any item again.
    if let Some(code) = halt
        && !checks.readbacks.is_empty()
    {
        return Err(Failure::ReadbackPending(format!(
            "the provider halted the batch ({code}); its applied items are read back when it is retried"
        )));
    }
    let mut confirmed = Vec::new();
    let mut evidence = Vec::new();
    for (index, outcome) in outcomes.iter().enumerate() {
        let Some(applied) = outcome
            .as_object()
            .filter(|outcome| outcome.get("status").and_then(JsonValue::as_str) == Some("applied"))
            .and_then(|outcome| outcome.get("acknowledgement"))
        else {
            continue;
        };
        for (readback, matches) in checks.readbacks.iter().enumerate() {
            let observed = driver
                .check(
                    Check::Readback {
                        readback,
                        item: Some(index),
                    },
                    applied,
                )
                .map_err(Failure::ReadbackPending)?;
            verify(
                matches,
                call.arguments,
                items.get(index),
                applied,
                &observed.value,
            )
            .map_err(Failure::ReadbackPending)?;
            evidence.extend(observed.evidence);
            confirmed.push(observed.value);
        }
    }
    Ok((JsonValue::Array(outcomes), confirmed, evidence))
}

fn preconditions(
    driver: &dyn Driver,
    call: &AdmittedCall<'_>,
    checks: &Checks,
) -> Result<(), Failure> {
    for (index, matches) in checks.preconditions.iter().enumerate() {
        driver
            .check(Check::Precondition(index), &JsonValue::Null)
            .and_then(|observed| {
                verify(
                    matches,
                    call.arguments,
                    None,
                    &JsonValue::Null,
                    &observed.value,
                )
            })
            .map_err(|reason| rejected("precondition_failed", reason))?;
    }
    Ok(())
}

/// One item's outcome in a repeated write: its status and the facts present.
fn outcome<'a>(
    status: &str,
    facts: impl IntoIterator<Item = (&'a str, Option<JsonValue>)>,
) -> JsonValue {
    let mut outcome =
        JsonObject::from([("status".to_owned(), JsonValue::String(status.to_owned()))]);
    for (name, value) in facts {
        if let Some(value) = value {
            outcome.insert(name.to_owned(), value);
        }
    }
    JsonValue::Object(outcome)
}

fn refused<'a>(
    code: &str,
    reason: String,
    facts: impl IntoIterator<Item = (&'a str, Option<JsonValue>)>,
) -> JsonValue {
    outcome(
        "refused",
        [
            ("code", Some(JsonValue::String(code.to_owned()))),
            ("reason", Some(JsonValue::String(reason))),
        ]
        .into_iter()
        .chain(facts),
    )
}

fn acknowledgement(write_result: JsonValue) -> Acknowledgement {
    Acknowledgement {
        response: JsonObject::new(),
        write_result,
    }
}

/// A confirming read matches the acknowledged write, the arguments the call
/// was approved with and, for a repeated write, the item it was sent for.
fn verify(
    matches: &[ReadbackMatch],
    arguments: &JsonValue,
    item: Option<&JsonValue>,
    write: &JsonValue,
    observed: &JsonValue,
) -> Result<(), String> {
    for binding in matches {
        let expected = match &binding.expected {
            ReadbackExpected::Acknowledgement { path } => at_path(write, path)
                .filter(|value| !matches!(value, JsonValue::Null))
                .ok_or("the acknowledgement identity is missing")?,
            ReadbackExpected::Argument { path } => match at_path(arguments, path) {
                Some(value) => value,
                None => continue,
            },
            ReadbackExpected::Item { path } => {
                let item = item.ok_or("an item match needs a repeated write")?;
                match at_path(item, path) {
                    Some(value) => value,
                    None => continue,
                }
            }
            ReadbackExpected::Literal { value } => value,
        };
        if !observed_matches(observed, &binding.observed, expected) {
            return Err("the readback does not match the acknowledged write".to_owned());
        }
    }
    Ok(())
}

/// Whether the value at an observed path is `expected`. A `*` segment maps
/// over an array, and the values it gathers compare with `expected` as a set,
/// as a provider returns labels as objects in its own order. A path that
/// passes through null is null, as a cleared field reads back.
fn observed_matches(observed: &JsonValue, path: &str, expected: &JsonValue) -> bool {
    let segments = path.split('.').collect::<Vec<_>>();
    match project(observed, &segments) {
        Some(found) if segments.contains(&"*") => match (found, expected) {
            (JsonValue::Array(found), JsonValue::Array(expected)) => {
                as_set(&found) == as_set(expected)
            }
            _ => false,
        },
        Some(found) => &found == expected,
        None => false,
    }
}

fn project(value: &JsonValue, segments: &[&str]) -> Option<JsonValue> {
    let Some((segment, rest)) = segments.split_first() else {
        return Some(value.clone());
    };
    match (*segment, value) {
        ("*", JsonValue::Array(values)) => values
            .iter()
            .map(|value| project(value, rest))
            .collect::<Option<Vec<_>>>()
            .map(JsonValue::Array),
        (_, JsonValue::Null) => Some(JsonValue::Null),
        (key, JsonValue::Object(object)) => project(object.get(key)?, rest),
        (index, JsonValue::Array(values)) => {
            project(values.get(index.parse::<usize>().ok()?)?, rest)
        }
        _ => None,
    }
}

fn as_set(values: &[JsonValue]) -> std::collections::BTreeSet<Vec<u8>> {
    values
        .iter()
        .filter_map(|value| runx_contracts::to_canonical_bytes(value).ok())
        .collect()
}

/// The value at a dotted path; array segments are indices.
pub(crate) fn at_path<'a>(value: &'a JsonValue, path: &str) -> Option<&'a JsonValue> {
    path.split('.')
        .try_fold(value, |current, segment| match current {
            JsonValue::Object(object) => object.get(segment),
            JsonValue::Array(values) => segment
                .parse::<usize>()
                .ok()
                .and_then(|index| values.get(index)),
            _ => None,
        })
}

fn rejected(code: &str, reason: String) -> Failure {
    Failure::Rejected {
        status: 0,
        code: code.to_owned(),
        reason,
        release: true,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use std::cell::{Cell, RefCell};
    use std::collections::BTreeMap;

    use runx_contracts::JsonObject;
    use runx_contracts::connector::OperationAccess;

    use super::super::Sealed;
    use super::*;

    struct Fake {
        performed: Cell<usize>,
        answer: fn() -> Result<Observed, Unperformed>,
        observed: JsonValue,
    }

    impl Driver for Fake {
        fn perform(&self, _item: Option<usize>) -> Result<Observed, Unperformed> {
            self.performed.set(self.performed.get() + 1);
            (self.answer)()
        }

        fn check(&self, _check: Check, _write: &JsonValue) -> Result<Observed, String> {
            Ok(Observed {
                value: self.observed.clone(),
                evidence: vec!["sha256:read".to_owned()],
            })
        }

        fn result(
            &self,
            write: &JsonValue,
            _confirmed: &[JsonValue],
        ) -> Result<(JsonValue, Option<String>), String> {
            Ok((write.clone(), None))
        }
    }

    fn id(value: &str) -> JsonValue {
        JsonValue::Object(JsonObject::from([(
            "id".to_owned(),
            JsonValue::String(value.to_owned()),
        )]))
    }

    fn written() -> Result<Observed, Unperformed> {
        Ok(Observed {
            value: id("m-1"),
            evidence: Vec::new(),
        })
    }

    fn identity() -> Checks {
        Checks {
            preconditions: Vec::new(),
            readbacks: vec![vec![ReadbackMatch {
                expected: ReadbackExpected::Acknowledgement {
                    path: "id".to_owned().into(),
                },
                observed: "id".to_owned().into(),
            }]],
        }
    }

    fn admitted<T>(access: OperationAccess, run: impl FnOnce(&AdmittedCall<'_>) -> T) -> T {
        let arguments = JsonValue::Object(JsonObject::new());
        let parameters = JsonObject::new();
        let env = BTreeMap::new();
        run(&AdmittedCall {
            connector: "example",
            operation: "item.write",
            implementation: "http",
            target: "example://item",
            arguments: &arguments,
            parameters: &parameters,
            idempotency_key: Some("key-1"),
            credential: None,
            env: &env,
            access,
            _sealed: Sealed,
        })
    }

    #[test]
    fn a_providers_failure_never_repeats_the_presented_credential() {
        let secret = CredentialSecret::new(
            runx_contracts::connector::CredentialName::new("key").expect("a credential name"),
            BTreeMap::from([("token".to_owned(), "sk_live_private".to_owned())]),
        );
        let driver = Fake {
            performed: Cell::new(0),
            answer: || {
                Err(Unperformed::Refused {
                    code: "tool_error",
                    reason: "tool post reported an error: token sk_live_private is revoked"
                        .to_owned(),
                    answer: None,
                })
            },
            observed: JsonValue::Null,
        };
        let arguments = JsonValue::Object(JsonObject::new());
        let parameters = JsonObject::new();
        let env = BTreeMap::new();
        let call = AdmittedCall {
            connector: "example",
            operation: "item.write",
            implementation: "mcp",
            target: "example://item",
            arguments: &arguments,
            parameters: &parameters,
            idempotency_key: Some("key-1"),
            credential: Some(&secret),
            env: &env,
            access: OperationAccess::Mutate,
            _sealed: Sealed,
        };

        let failure = run(
            &driver,
            &call,
            &Checks::default(),
            Recovery {
                acknowledged: None,
                resend_safe: false,
            },
            &mut |_| Ok(()),
        );

        let reason = match failure {
            Err(Failure::Rejected { reason, .. }) => Some(reason),
            _ => None,
        }
        .expect("a refused write is rejected");
        assert!(!reason.contains("sk_live_private"), "{reason}");
        assert!(reason.contains("[redacted-credential]"), "{reason}");
    }

    #[test]
    fn a_recorded_acknowledgement_is_read_back_and_never_performed_again() {
        let driver = Fake {
            performed: Cell::new(0),
            answer: written,
            observed: id("m-1"),
        };
        let recorded = Acknowledgement {
            response: JsonObject::new(),
            write_result: id("m-1"),
        };
        let readback = admitted(OperationAccess::Mutate, |call| {
            run(
                &driver,
                call,
                &identity(),
                Recovery {
                    acknowledged: Some(recorded),
                    resend_safe: false,
                },
                &mut |_| Ok(()),
            )
        });
        assert_eq!(driver.performed.get(), 0);
        assert!(matches!(readback, Ok(readback) if readback.result == id("m-1")));
    }

    #[test]
    fn a_write_that_may_have_reached_the_provider_is_never_refused() {
        let driver = Fake {
            performed: Cell::new(0),
            answer: || {
                Err(Unperformed::MaybeSent {
                    code: "transport_failed",
                    reason: "connection reset".to_owned(),
                })
            },
            observed: id("m-1"),
        };
        let write = admitted(OperationAccess::Mutate, |call| {
            run(
                &driver,
                call,
                &identity(),
                Recovery {
                    acknowledged: None,
                    resend_safe: false,
                },
                &mut |_| Ok(()),
            )
        });
        assert!(matches!(write, Err(Failure::Unknown(_))));
        let read = admitted(OperationAccess::Read, |call| {
            run(
                &driver,
                call,
                &Checks::default(),
                Recovery {
                    acknowledged: None,
                    resend_safe: false,
                },
                &mut |_| Ok(()),
            )
        });
        assert!(matches!(read, Err(Failure::Rejected { code, .. }) if code == "transport_failed"));
    }

    #[test]
    fn a_readback_is_checked_against_the_arguments_the_call_carries() {
        let arguments = JsonValue::Object(JsonObject::from([(
            "title".to_owned(),
            JsonValue::String("approved".to_owned()),
        )]));
        let title = |path: &str| ReadbackMatch {
            expected: ReadbackExpected::Argument {
                path: path.to_owned().into(),
            },
            observed: "title".to_owned().into(),
        };
        let observed = |value: &str| {
            JsonValue::Object(JsonObject::from([(
                "title".to_owned(),
                JsonValue::String(value.to_owned()),
            )]))
        };
        assert!(
            verify(
                &[title("title")],
                &arguments,
                None,
                &JsonValue::Null,
                &observed("approved")
            )
            .is_ok()
        );
        assert!(
            verify(
                &[title("title")],
                &arguments,
                None,
                &JsonValue::Null,
                &observed("other")
            )
            .is_err()
        );
        assert!(
            verify(
                &[title("body")],
                &arguments,
                None,
                &JsonValue::Null,
                &observed("other")
            )
            .is_ok()
        );
    }

    #[test]
    fn a_starred_observed_path_compares_what_it_gathers_as_a_set() {
        let json = |value: serde_json::Value| {
            serde_json::from_value::<JsonValue>(value).expect("a JSON value")
        };
        let issue = json(serde_json::json!({
            "state": "open",
            "labels": [{"name": "triage"}, {"name": "bug"}],
        }));
        assert!(observed_matches(
            &issue,
            "labels.*.name",
            &json(serde_json::json!(["bug", "triage"]))
        ));
        assert!(!observed_matches(
            &issue,
            "labels.*.name",
            &json(serde_json::json!(["bug"]))
        ));
        assert!(!observed_matches(
            &issue,
            "labels.*.name",
            &json(serde_json::json!("bug"))
        ));
        assert!(observed_matches(
            &issue,
            "state",
            &json(serde_json::json!("open"))
        ));
        assert!(!observed_matches(
            &issue,
            "assignees.*.login",
            &json(serde_json::json!([]))
        ));
        let cleared = json(serde_json::json!({"milestone": null}));
        assert!(observed_matches(
            &cleared,
            "milestone.number",
            &JsonValue::Null
        ));
        assert!(!observed_matches(
            &cleared,
            "milestone.number",
            &json(serde_json::json!(3))
        ));
        let set = json(serde_json::json!({"milestone": {"number": 3}}));
        assert!(observed_matches(
            &set,
            "milestone.number",
            &json(serde_json::json!(3))
        ));
    }

    #[test]
    fn an_acknowledgement_is_recorded_before_a_readback_that_must_match_it() {
        let driver = Fake {
            performed: Cell::new(0),
            answer: written,
            observed: id("m-2"),
        };
        let recorded = RefCell::new(Vec::new());
        let outcome = admitted(OperationAccess::Mutate, |call| {
            run(
                &driver,
                call,
                &identity(),
                Recovery {
                    acknowledged: None,
                    resend_safe: false,
                },
                &mut |acknowledgement| {
                    recorded
                        .borrow_mut()
                        .push(acknowledgement.write_result.clone());
                    Ok(())
                },
            )
        });
        assert_eq!(recorded.borrow().as_slice(), &[id("m-1")]);
        assert!(matches!(outcome, Err(Failure::ReadbackPending(_))));
    }

    /// Items `a`, `b` and `c`, where the provider refuses `b`, or rate limits
    /// the caller at `b` when `halts`.
    struct Batch {
        performed: RefCell<Vec<usize>>,
        halts: bool,
    }

    impl Driver for Batch {
        fn items(&self) -> Result<Option<Vec<JsonValue>>, String> {
            Ok(Some(
                ["a", "b", "c"]
                    .map(|name| JsonValue::String(name.to_owned()))
                    .to_vec(),
            ))
        }

        fn perform(&self, item: Option<usize>) -> Result<Observed, Unperformed> {
            let item = item.expect("a repeated write performs items");
            self.performed.borrow_mut().push(item);
            match item {
                1 if self.halts => Err(Unperformed::Halted {
                    code: "rate_limited",
                    reason: "rate limited".to_owned(),
                    retry_at: Some("2027-01-15T08:00:00Z".to_owned()),
                }),
                1 => Err(Unperformed::Refused {
                    code: "not_found",
                    reason: "no such account".to_owned(),
                    answer: None,
                }),
                _ => Ok(Observed {
                    value: id(&format!("m-{item}")),
                    evidence: Vec::new(),
                }),
            }
        }

        fn check(&self, check: Check, write: &JsonValue) -> Result<Observed, String> {
            assert!(matches!(check, Check::Readback { item: Some(_), .. }));
            Ok(Observed {
                value: write.clone(),
                evidence: vec!["sha256:read".to_owned()],
            })
        }

        fn result(
            &self,
            write: &JsonValue,
            _confirmed: &[JsonValue],
        ) -> Result<(JsonValue, Option<String>), String> {
            Ok((write.clone(), None))
        }
    }

    fn statuses(outcomes: &JsonValue) -> Vec<&str> {
        outcomes
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|outcome| {
                outcome
                    .as_object()
                    .and_then(|outcome| outcome.get("status"))
                    .and_then(JsonValue::as_str)
            })
            .collect()
    }

    #[test]
    fn a_batch_records_each_outcome_reads_back_what_applied_and_resumes_after_the_last() {
        let driver = Batch {
            performed: RefCell::new(Vec::new()),
            halts: false,
        };
        let recorded = RefCell::new(Vec::new());
        let readback = admitted(OperationAccess::Mutate, |call| {
            run(
                &driver,
                call,
                &identity(),
                Recovery {
                    acknowledged: None,
                    resend_safe: false,
                },
                &mut |acknowledgement| {
                    recorded
                        .borrow_mut()
                        .push(acknowledgement.write_result.clone());
                    Ok(())
                },
            )
        })
        .expect("a batch with a refused item is settled");
        assert_eq!(
            statuses(&readback.result),
            ["applied", "refused", "applied"]
        );
        assert_eq!(
            recorded
                .borrow()
                .iter()
                .map(|outcomes| statuses(outcomes).len())
                .collect::<Vec<_>>(),
            [1, 2, 3],
            "every outcome is recorded as it lands"
        );

        let first = recorded.borrow()[0].clone();
        let resumed = Batch {
            performed: RefCell::new(Vec::new()),
            halts: false,
        };
        let held = admitted(OperationAccess::Mutate, |call| {
            run(
                &resumed,
                call,
                &identity(),
                Recovery {
                    acknowledged: Some(Acknowledgement {
                        response: JsonObject::new(),
                        write_result: first.clone(),
                    }),
                    resend_safe: false,
                },
                &mut |_| Ok(()),
            )
        });
        assert!(
            matches!(held, Err(Failure::Unknown(_))),
            "item 2 may have landed"
        );
        assert!(resumed.performed.borrow().is_empty());

        let safe = admitted(OperationAccess::Mutate, |call| {
            run(
                &resumed,
                call,
                &identity(),
                Recovery {
                    acknowledged: Some(Acknowledgement {
                        response: JsonObject::new(),
                        write_result: first,
                    }),
                    resend_safe: true,
                },
                &mut |_| Ok(()),
            )
        });
        assert!(safe.is_ok());
        assert_eq!(
            *resumed.performed.borrow(),
            [1, 2],
            "a resend-safe batch resumes after its last record"
        );
    }

    #[test]
    fn a_halted_batch_sends_nothing_more_and_reads_back_only_on_the_retry() {
        let driver = Batch {
            performed: RefCell::new(Vec::new()),
            halts: true,
        };
        let recorded = RefCell::new(None);
        let halted = admitted(OperationAccess::Mutate, |call| {
            run(
                &driver,
                call,
                &identity(),
                Recovery {
                    acknowledged: None,
                    resend_safe: false,
                },
                &mut |acknowledgement| {
                    *recorded.borrow_mut() = Some(acknowledgement.write_result.clone());
                    Ok(())
                },
            )
        });
        assert!(
            matches!(halted, Err(Failure::ReadbackPending(_))),
            "a halt defers the checking reads"
        );
        assert_eq!(*driver.performed.borrow(), [0, 1], "item c is never sent");
        let outcomes = recorded
            .borrow()
            .clone()
            .expect("the outcomes are recorded");
        assert_eq!(statuses(&outcomes), ["applied", "refused", "unsent"]);
        assert_eq!(
            outcomes
                .as_array()
                .and_then(|outcomes| outcomes.get(1))
                .and_then(JsonValue::as_object)
                .and_then(|outcome| outcome.get("retry_at"))
                .and_then(JsonValue::as_str),
            Some("2027-01-15T08:00:00Z")
        );

        let retried = Batch {
            performed: RefCell::new(Vec::new()),
            halts: true,
        };
        let readback = admitted(OperationAccess::Mutate, |call| {
            run(
                &retried,
                call,
                &identity(),
                Recovery {
                    acknowledged: Some(Acknowledgement {
                        response: JsonObject::new(),
                        write_result: outcomes,
                    }),
                    resend_safe: false,
                },
                &mut |_| Ok(()),
            )
        })
        .expect("the retry reads back what applied");
        assert!(
            retried.performed.borrow().is_empty(),
            "nothing is sent again"
        );
        assert_eq!(statuses(&readback.result), ["applied", "refused", "unsent"]);
    }
}
