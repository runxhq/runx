//! The protocol every local driver runs under, and the readback check.
//!
//! A driver performs what it is handed and reports what it saw. The kernel
//! decides the rest, once for every implementation: a write with a recorded
//! acknowledgement is never performed again, an acknowledgement is recorded
//! in custody before anything is read back, every confirming read must match
//! the acknowledged write, a write that may have reached the provider is
//! never reported as refused, and a write's evidence is its confirming reads,
//! never its own answer. An answer that contradicts what was asked, a
//! write's own or a confirming read's, settles the call failed, naming what
//! the provider holds; a read that doesn't answer, or states no value where
//! the call is compared, leaves the outcome unknown. A write the provider
//! cannot read back declares no readbacks: its evidence is its recorded
//! acknowledgement, which the receipt seals as evidence of the effect, never
//! as its finality.
//!
//! A repeated write is one approved call over an exact list of items. Each
//! item's outcome is recorded as it lands, a refused item does not stop the
//! others, a provider that refuses the caller (a rate limit) settles every
//! later item as unsent and defers the checking reads to the retry, an item
//! that may have landed stops the batch as an unknown outcome, one whose
//! answer contradicts the call settles it failed, and every applied item is
//! read back. A recovered batch resumes after its last
//! recorded item.

use runx_contracts::connector::{ReadbackExpected, ReadbackMatch};
use runx_contracts::{JsonObject, JsonValue, canonical_stable_json, sha256_prefixed};

use super::credentials::CredentialSecret;
use super::packet::Readback;
use super::{AdmittedCall, Failure};
use crate::error::{Contradiction, Mismatch};
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
        /// Earlier requests in this operation completed before the halt.
        /// A refusal of this request cannot settle their possible charges.
        partial: bool,
    },
    /// The request may have reached the provider, and nothing shows that it
    /// did not act.
    MaybeSent { code: &'static str, reason: String },
    /// The provider answered that it did other than the call asked: other
    /// bytes, another key or another principal. It may have acted, and the
    /// same call gets the same answer.
    Contradicted(Contradiction),
}

impl Unperformed {
    pub(crate) fn reason(self) -> String {
        match self {
            Self::NotSent { reason, .. }
            | Self::Refused { reason, .. }
            | Self::Halted { reason, .. }
            | Self::MaybeSent { reason, .. } => reason,
            Self::Contradicted(contradiction) => contradiction.reason(),
        }
    }
}

/// Why a checking read didn't confirm the call.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Unconfirmed {
    /// It answered, and contradicts what was asked.
    Contradicted(Contradiction),
    /// It didn't answer what was asked: a timeout, a lost connection, a
    /// provider error, or an answer that states nothing where the call is
    /// compared. The outcome stays unknown.
    Unanswered(String),
}

impl From<String> for Unconfirmed {
    fn from(reason: String) -> Self {
        Self::Unanswered(reason)
    }
}

impl From<&str> for Unconfirmed {
    fn from(reason: &str) -> Self {
        Self::Unanswered(reason.to_owned())
    }
}

impl From<Unperformed> for Unconfirmed {
    fn from(unperformed: Unperformed) -> Self {
        Self::Unanswered(unperformed.reason())
    }
}

impl Unconfirmed {
    /// The call's failure: settled for a contradiction, else `unanswered`.
    pub(crate) fn failure(self, unanswered: impl FnOnce(String) -> Failure) -> Failure {
        match self {
            Self::Contradicted(contradiction) => Failure::Contradicted(contradiction),
            Self::Unanswered(reason) => unanswered(reason),
        }
    }

    pub(crate) fn reason(self) -> String {
        match self {
            Self::Contradicted(contradiction) => contradiction.reason(),
            Self::Unanswered(reason) => reason,
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
    /// A missing response may leave a provider charge even for a read.
    pub may_charge: bool,
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
    /// precondition sees null. A read that answers with what contradicts the
    /// call in what the driver itself compares reports it as contradicted.
    fn check(&self, check: Check, write: &JsonValue) -> Result<Observed, Unconfirmed>;

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
                    partial,
                } => Unperformed::Halted {
                    code,
                    reason: self.text(reason),
                    retry_at,
                    partial,
                },
                Unperformed::MaybeSent { code, reason } => Unperformed::MaybeSent {
                    code,
                    reason: self.text(reason),
                },
                Unperformed::Contradicted(contradiction) => {
                    Unperformed::Contradicted(match self.credential {
                        Some(secret) => contradiction.scrub(secret),
                        None => contradiction,
                    })
                }
            })
    }

    fn check(&self, check: Check, write: &JsonValue) -> Result<Observed, Unconfirmed> {
        self.driver
            .check(check, write)
            .map(|observed| self.observed(observed))
            .map_err(|unconfirmed| match (unconfirmed, self.credential) {
                (Unconfirmed::Contradicted(contradiction), Some(secret)) => {
                    Unconfirmed::Contradicted(contradiction.scrub(secret))
                }
                (Unconfirmed::Unanswered(reason), _) => Unconfirmed::Unanswered(self.text(reason)),
                (contradicted, None) => contradicted,
            })
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
    let may_charge = recovery.may_charge;
    let (write, confirmed, evidence) = match items {
        None => once(driver, call, checks, recovery, record)?,
        Some(items) => repeated(driver, call, checks, &items, recovery, record)?,
    };
    let (result, operation_id) = driver.result(&write, &confirmed).map_err(|reason| {
        match (call.mutation(), may_charge) {
            (true, _) => Failure::ReadbackPending(reason),
            (false, true) => Failure::Unknown(reason),
            (false, false) => rejected("invalid_response", reason),
        }
    })?;
    let readback_uri = if call.mutation() && checks.readbacks.is_empty() {
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
        readback_uri,
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
            preconditions(driver, call, checks, recovery.may_charge)?;
            let performed = driver.perform(None).map_err(|failure| match failure {
                // Nothing of this call left, but a priced check before it may have been charged.
                Unperformed::NotSent { code, reason } => Failure::Rejected {
                    status: 0,
                    code: code.to_owned(),
                    reason,
                    release: !recovery.may_charge || checks.preconditions.is_empty(),
                    conflict: None,
                },
                Unperformed::Halted {
                    reason,
                    partial: true,
                    ..
                } if recovery.may_charge => Failure::Unknown(reason),
                Unperformed::Halted {
                    code,
                    reason,
                    partial,
                    ..
                } => Failure::Rejected {
                    status: 0,
                    code: code.to_owned(),
                    reason,
                    release: !partial && !recovery.may_charge,
                    conflict: None,
                },
                Unperformed::Refused { code, reason, .. } => Failure::Rejected {
                    status: 0,
                    code: code.to_owned(),
                    reason,
                    release: !recovery.may_charge,
                    conflict: None,
                },
                Unperformed::MaybeSent { reason, .. } if mutation || recovery.may_charge => {
                    Failure::Unknown(reason)
                }
                Unperformed::MaybeSent { code, reason } => Failure::Rejected {
                    status: 0,
                    code: code.to_owned(),
                    reason,
                    release: false,
                    conflict: None,
                },
                Unperformed::Contradicted(contradiction) => Failure::Contradicted(contradiction),
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
            .and_then(|observed| {
                verify(
                    matches,
                    call.arguments,
                    call.withheld,
                    None,
                    &write,
                    &observed.value,
                )
                .map(|()| observed)
            })
            .map_err(|unconfirmed| unconfirmed.failure(Failure::ReadbackPending))?;
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
            preconditions(driver, call, checks, recovery.may_charge)?;
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
                ..
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
            // An item whose answer contradicts the call settles the batch
            // failed where it stands, as one call's answer would.
            Err(Unperformed::Contradicted(contradiction)) => {
                return Err(Failure::Contradicted(contradiction));
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
                .and_then(|observed| {
                    verify(
                        matches,
                        call.arguments,
                        call.withheld,
                        items.get(index),
                        applied,
                        &observed.value,
                    )
                    .map(|()| observed)
                })
                .map_err(|unconfirmed| unconfirmed.failure(Failure::ReadbackPending))?;
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
    may_charge: bool,
) -> Result<(), Failure> {
    for (index, matches) in checks.preconditions.iter().enumerate() {
        driver
            .check(Check::Precondition(index), &JsonValue::Null)
            .and_then(|observed| {
                verify(
                    matches,
                    call.arguments,
                    call.withheld,
                    None,
                    &JsonValue::Null,
                    &observed.value,
                )
            })
            .map_err(|unconfirmed| Failure::Rejected {
                status: 0,
                code: "precondition_failed".to_owned(),
                reason: unconfirmed.reason(),
                // A timed-out read may still have been charged, and a failed
                // check does not say otherwise, so a priced call keeps its spend.
                release: !may_charge,
                conflict: None,
            })?;
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
/// was approved with and, for a repeated write, the item it was sent for. A
/// field it states with another value contradicts the call, whatever else
/// can't be compared.
/// `withheld` holds the result paths the call marks never to persist.
fn verify(
    matches: &[ReadbackMatch],
    arguments: &JsonValue,
    withheld: &[Vec<String>],
    item: Option<&JsonValue>,
    write: &JsonValue,
    observed: &JsonValue,
) -> Result<(), Unconfirmed> {
    let responses = std::cell::OnceCell::new();
    let mut mismatches = Vec::new();
    let mut unanswered = None;
    for binding in matches {
        match compare(
            binding, &responses, arguments, withheld, item, write, observed,
        ) {
            Ok(()) => {}
            Err(Unconfirmed::Contradicted(contradiction)) => {
                mismatches.extend(contradiction.mismatches);
            }
            Err(Unconfirmed::Unanswered(reason)) => {
                unanswered.get_or_insert(reason);
            }
        }
    }
    match (mismatches.is_empty(), unanswered) {
        (false, _) => Err(Unconfirmed::Contradicted(Contradiction { mismatches })),
        (true, Some(reason)) => Err(Unconfirmed::Unanswered(reason)),
        (true, None) => Ok(()),
    }
}

/// One match of a confirming read. A field it states no value at, absent or
/// null, doesn't answer the call; one it states another value at
/// contradicts it.
fn compare(
    binding: &ReadbackMatch,
    responses: &std::cell::OnceCell<std::collections::BTreeMap<String, JsonObject>>,
    arguments: &JsonValue,
    withheld: &[Vec<String>],
    item: Option<&JsonValue>,
    write: &JsonValue,
    observed: &JsonValue,
) -> Result<(), Unconfirmed> {
    let calculated;
    let expected = match &binding.expected {
        ReadbackExpected::Acknowledgement { path } => at_path(write, path)
            .filter(|value| !matches!(value, JsonValue::Null))
            .ok_or("the acknowledgement identity is missing")?,
        ReadbackExpected::Argument { path } => match at_path(arguments, path) {
            Some(value) => value,
            None => return Ok(()),
        },
        ReadbackExpected::Item { path } => {
            let item = item.ok_or("an item match needs a repeated write")?;
            match at_path(item, path) {
                Some(value) => value,
                None => return Ok(()),
            }
        }
        ReadbackExpected::Literal { value } => value,
        ReadbackExpected::Template { value } => {
            let responses = responses.get_or_init(|| {
                std::collections::BTreeMap::from([(
                    "write".to_owned(),
                    JsonObject::from([("json".to_owned(), write.clone())]),
                )])
            });
            calculated = value
                .render(&runx_contracts::connector::TemplateInputs {
                    arguments: Some(arguments),
                    responses: Some(responses),
                    ..runx_contracts::connector::TemplateInputs::default()
                })
                .map_err(|_| "the readback's expected terms cannot be resolved")?;
            &calculated
        }
    };
    let path = binding.observed.as_str();
    let held =
        held_at(observed, path).ok_or_else(|| format!("the readback states nothing at {path}"))?;
    if holds(&held, path, expected) {
        return Ok(());
    }
    if held == JsonValue::Null {
        return Err(format!("the readback states no value at {path}").into());
    }
    Err(Unconfirmed::Contradicted(Contradiction {
        mismatches: vec![Mismatch::new(path, expected.clone(), Some(held), withheld)?],
    }))
}

/// What a read states at an observed path. A `*` segment maps over an
/// array, and a path that passes through null is null, as a cleared field
/// reads back.
fn held_at(observed: &JsonValue, path: &str) -> Option<JsonValue> {
    project(observed, &path.split('.').collect::<Vec<_>>())
}

/// Whether what a read states at `path` is `expected`. The values a `*`
/// segment gathers compare with `expected` as a set, as a provider returns
/// labels as objects in its own order.
fn holds(held: &JsonValue, path: &str, expected: &JsonValue) -> bool {
    if path.split('.').any(|segment| segment == "*") {
        return match (held, expected) {
            (JsonValue::Array(held), JsonValue::Array(expected)) => {
                as_set(held) == as_set(expected)
            }
            _ => false,
        };
    }
    held == expected
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
        conflict: None,
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
    use crate::error::Recorded;

    fn observed_matches(observed: &JsonValue, path: &str, expected: &JsonValue) -> bool {
        held_at(observed, path).is_some_and(|held| holds(&held, path, expected))
    }

    #[test]
    fn structured_relationships_bind_the_original_resource_independently_of_link_order() {
        let parse =
            |value: serde_json::Value| serde_json::from_value::<JsonValue>(value).expect("JSON");
        let expected: ReadbackMatch = serde_json::from_value(serde_json::json!({
            "expected":{"source":"template","value":[
                {"rel":"self","method":"GET","href":{"$concat":["https://api.example.test/refunds/",{"$response":"write.json.refund_id"}]}},
                {"rel":"up","method":"GET","href":{"$concat":["https://api.example.test/captures/",{"$argument":"/payment_id"}]}}
            ]},"observed":"relationships.*"
        })).expect("match");
        let arguments = parse(serde_json::json!({"payment_id":"ORIGINAL"}));
        let write = parse(serde_json::json!({"refund_id":"REFUND1"}));
        let self_link = serde_json::json!({"rel":"self","method":"GET","href":"https://api.example.test/refunds/REFUND1"});
        let original_link = serde_json::json!({"rel":"up","method":"GET","href":"https://api.example.test/captures/ORIGINAL"});
        let matching = parse(serde_json::json!({"relationships":[original_link, self_link]}));
        assert!(
            verify(
                std::slice::from_ref(&expected),
                &arguments,
                &[],
                None,
                &write,
                &matching
            )
            .is_ok()
        );
        for wrong in [
            serde_json::json!({"rel":"up","method":"GET","href":"https://api.example.test/captures/OTHER"}),
            serde_json::json!({"rel":"self","method":"GET","href":"https://api.example.test/captures/ORIGINAL"}),
            serde_json::json!({"rel":"up","method":"POST","href":"https://api.example.test/captures/ORIGINAL"}),
        ] {
            let observed = parse(serde_json::json!({"relationships":[self_link,wrong]}));
            assert!(
                verify(
                    std::slice::from_ref(&expected),
                    &arguments,
                    &[],
                    None,
                    &write,
                    &observed
                )
                .is_err()
            );
        }
        assert!(
            verify(
                &[expected],
                &parse(serde_json::json!({})),
                &[],
                None,
                &write,
                &matching
            )
            .is_err(),
            "missing approved terms are never skipped"
        );
    }

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

        fn check(&self, _check: Check, _write: &JsonValue) -> Result<Observed, Unconfirmed> {
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
            session_principal: None,
            env: &env,
            held: Vec::new(),
            writer: None,
            withheld: &[],
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
            session_principal: None,
            env: &env,
            held: Vec::new(),
            writer: None,
            withheld: &[],
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
                may_charge: false,
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
                    may_charge: false,
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
                    may_charge: false,
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
                    may_charge: false,
                },
                &mut |_| Ok(()),
            )
        });
        assert!(matches!(read, Err(Failure::Rejected { code, .. }) if code == "transport_failed"));
        let charged_read = admitted(OperationAccess::Read, |call| {
            run(
                &driver,
                call,
                &Checks::default(),
                Recovery {
                    acknowledged: None,
                    resend_safe: false,
                    may_charge: true,
                },
                &mut |_| Ok(()),
            )
        });
        assert!(matches!(charged_read, Err(Failure::Unknown(_))));
    }

    #[test]
    fn a_halt_after_partial_read_keeps_prior_uses_and_uncertain_charges() {
        let driver = Fake {
            performed: Cell::new(0),
            answer: || {
                Err(Unperformed::Halted {
                    code: "rate_limited",
                    reason: "one request completed before the rate limit".to_owned(),
                    retry_at: Some("2027-01-15T08:00:00Z".to_owned()),
                    partial: true,
                })
            },
            observed: id("m-1"),
        };
        for may_charge in [false, true] {
            let outcome = admitted(OperationAccess::Read, |call| {
                run(
                    &driver,
                    call,
                    &Checks::default(),
                    Recovery {
                        acknowledged: None,
                        resend_safe: false,
                        may_charge,
                    },
                    &mut |_| Ok(()),
                )
            });
            if may_charge {
                assert!(matches!(outcome, Err(Failure::Unknown(_))));
            } else {
                assert!(matches!(
                    outcome,
                    Err(Failure::Rejected { release: false, .. })
                ));
            }
        }
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
                &[],
                None,
                &JsonValue::Null,
                &observed("approved")
            )
            .is_ok()
        );
        assert!(matches!(
            verify(
                &[title("title")],
                &arguments,
                &[],
                None,
                &JsonValue::Null,
                &observed("other")
            ),
            Err(Unconfirmed::Contradicted(_))
        ));
        assert!(
            matches!(
                verify(
                    &[title("title")],
                    &arguments,
                    &[],
                    None,
                    &JsonValue::Null,
                    &JsonValue::Object(JsonObject::new())
                ),
                Err(Unconfirmed::Unanswered(_))
            ),
            "a read that states nothing at the field doesn't answer it"
        );
        assert!(
            matches!(
                verify(
                    &[title("title")],
                    &arguments,
                    &[],
                    None,
                    &JsonValue::Null,
                    &JsonValue::Object(JsonObject::from([("title".to_owned(), JsonValue::Null)]))
                ),
                Err(Unconfirmed::Unanswered(_))
            ),
            "null states no value, so it doesn't answer the field either"
        );
        assert!(
            verify(
                &[title("body")],
                &arguments,
                &[],
                None,
                &JsonValue::Null,
                &observed("other")
            )
            .is_ok()
        );
    }

    #[test]
    fn a_contradicted_field_the_call_never_persists_is_recorded_by_its_digest() {
        let secret = |value: &str| {
            JsonValue::Object(JsonObject::from([(
                "token".to_owned(),
                JsonValue::Object(JsonObject::from([(
                    "value".to_owned(),
                    JsonValue::String(value.to_owned()),
                )])),
            )]))
        };
        let token = ReadbackMatch {
            expected: ReadbackExpected::Acknowledgement {
                path: "token.value".to_owned().into(),
            },
            observed: "token.value".to_owned().into(),
        };
        let withheld = [vec!["token".to_owned(), "value".to_owned()]];
        let digest = |value: &str| {
            sha256_prefixed(
                &runx_contracts::to_canonical_bytes(&JsonValue::String(value.to_owned()))
                    .expect("canonical"),
            )
        };
        let contradiction = match verify(
            &[token],
            &JsonValue::Null,
            &withheld,
            None,
            &secret("asked-secret"),
            &secret("held-secret"),
        ) {
            Err(Unconfirmed::Contradicted(contradiction)) => Some(contradiction),
            _ => None,
        }
        .expect("another token contradicts the write");
        assert_eq!(
            contradiction.mismatches,
            [Mismatch {
                field: "token.value".to_owned(),
                asked: Recorded::Digest(digest("asked-secret")),
                held: Some(Recorded::Digest(digest("held-secret"))),
            }]
        );
        let reason = contradiction.reason();
        assert!(!reason.contains("secret\""), "{reason}");
        assert!(reason.contains(&digest("held-secret")), "{reason}");
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
                    may_charge: false,
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
        let contradiction = match outcome {
            Err(Failure::Contradicted(contradiction)) => Some(contradiction),
            _ => None,
        }
        .expect("a readback holding another id contradicts the write");
        assert_eq!(
            contradiction.mismatches,
            [Mismatch {
                field: "id".to_owned(),
                asked: Recorded::Value(JsonValue::String("m-1".to_owned())),
                held: Some(Recorded::Value(JsonValue::String("m-2".to_owned()))),
            }]
        );
        assert_eq!(
            contradiction.to_string(),
            r#"id holds "m-2" where "m-1" was asked"#
        );
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
                    partial: false,
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

        fn check(&self, check: Check, write: &JsonValue) -> Result<Observed, Unconfirmed> {
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
                    may_charge: false,
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
                    may_charge: false,
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
                    may_charge: false,
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
                    may_charge: false,
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
                    may_charge: false,
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
