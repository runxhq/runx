//! A connector's feed (the loop contract's E4), read a page at a time after a
//! cursor with no run around it. The read goes through the kernel's one
//! path: admitted under the caller's authority and the workspace's instance
//! of the connector, and carried out by the connector's own driver. It
//! records nothing. A feed's read is a safe read, so admitting it holds
//! nothing in custody and counts no use, and what it returns is its caller's
//! to keep: a poll that finds nothing leaves no trace, and what arrived is
//! recorded once, where its reader keeps it.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use runx_contracts::connector::{ConnectorFeed, FeedName};
use runx_contracts::{JsonObject, JsonValue};
use runx_parser::{ConnectorRequirement, OperationStepTarget, StepKind, WorkflowStep};

use super::{AttemptPlan, NoTools, OperationAdmission, execute};
use crate::effects::{EffectStepRequest, ResolvedEffectTarget, RuntimeEffectRegistry};
use crate::kernel::admission::AdmissionDecision;
use crate::kernel::connectors::{ConnectorScope, LoadedConnector};

/// The run a feed's read stands in for: none, as an inspection's does.
const FEED: &str = "feed";
static FEED_CLOCK: crate::RunClock = crate::RunClock::System;

/// One read of a connector's feed.
pub struct FeedRead<'a> {
    /// The connector, as the person names it: one in the workspace's reach,
    /// or its one installed version.
    pub connector: &'a str,
    pub feed: &'a str,
    /// Where the last page ended, as the provider gave it; none from the
    /// feed's start.
    pub cursor: Option<&'a JsonValue>,
    /// How long the provider may hold the read open while nothing arrives,
    /// within what its read declares it takes.
    pub wait: Option<Duration>,
    /// The authority the read is admitted under: the reader's own.
    pub authority: Option<&'a crate::RunAuthority>,
    pub env: &'a BTreeMap<String, String>,
    pub workspace: &'a Path,
}

/// One page of a feed.
#[derive(Clone, Debug, PartialEq)]
pub struct FeedPage {
    pub events: Vec<FeedEvent>,
    /// Where to read on from; none when the provider gave none.
    pub cursor: Option<JsonValue>,
    /// More events follow at once.
    pub more: bool,
    /// The provider can't replay from the cursor given.
    pub gap: bool,
}

/// One event of a feed, by the provider's id for it.
#[derive(Clone, Debug, PartialEq)]
pub struct FeedEvent {
    pub id: String,
    pub data: JsonValue,
}

/// Whether connector `connector`, as the person names it in `workspace`,
/// offers feed `feed`.
pub fn feed_offered(
    env: &BTreeMap<String, String>,
    workspace: &Path,
    connector: &str,
    feed: &str,
) -> Result<(), String> {
    offered(env, workspace, connector, feed).map(|_| ())
}

/// The connector the person names in `workspace`, and the name of its feed
/// `feed`, when it offers one.
fn offered(
    env: &BTreeMap<String, String>,
    workspace: &Path,
    connector: &str,
    feed: &str,
) -> Result<(Arc<LoadedConnector>, FeedName), String> {
    let paths = crate::paths::RunxPaths::resolve(env, workspace);
    let connector = ConnectorScope::in_project(workspace, &paths)
        .and_then(|scope| scope.connector(connector))
        .map_err(|error| error.to_string())?;
    let name = FeedName::new(feed)
        .filter(|name| connector.manifest.feeds.contains_key(name))
        .ok_or_else(|| format!("connector {} offers no feed {feed}", connector.name))?;
    Ok((connector, name))
}

/// Read one page of `read`'s feed through the kernel's one path.
pub(crate) fn read(
    effects: &RuntimeEffectRegistry,
    read: &FeedRead<'_>,
) -> Result<FeedPage, String> {
    let (connector, name) = offered(read.env, read.workspace, read.connector, read.feed)?;
    let feed = &connector.manifest.feeds[&name];
    let step = WorkflowStep {
        id: format!("feed.{}", read.feed),
        kind: StepKind::Operation(OperationStepTarget {
            requirement: connector.name.as_str().to_owned(),
            operation: Some(feed.operation.clone()),
            from: Vec::new(),
            // The person named the connector, so the requirement pins the
            // manifest the workspace reaches it at.
            declared: Some(ConnectorRequirement {
                operations: vec![feed.operation.clone()],
                manifest_digest: Some(connector.manifest_digest.clone()),
                choose: None,
                from: Vec::new(),
            }),
            loaded: None,
            claims: None,
        }),
        artifacts: None,
        outputs: None,
        inputs: JsonObject::from([(
            "inputs".to_owned(),
            JsonValue::Object(inputs(feed, &connector.manifest, read)),
        )]),
        bindings: Vec::new(),
        act: None,
        context_skills: Vec::new(),
        when: None,
        guard: Vec::new(),
        retry_unknown_outcomes: false,
        effects: None,
        reach: None,
    };
    // The read is its own root: its authority's delegations resolve to the
    // instances standing for them here, as a run's do where it starts.
    let authority = read
        .authority
        .cloned()
        .map(|authority| {
            crate::kernel::admission::resolved_authority(
                authority,
                read.workspace,
                read.env,
                read.workspace,
            )
        })
        .transpose()?;
    let admission = effects
        .admit(EffectStepRequest {
            step: &step,
            target: ResolvedEffectTarget { tool: None },
            inputs: &step.inputs,
            env: read.env,
            workflow_dir: read.workspace,
            run_id: FEED,
            run_authority: authority.as_ref(),
            held_by_effects: false,
            chain: &[],
            clock: &FEED_CLOCK,
            reconciliation: None,
        })
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("feed {} is read through no operation", read.feed))?;
    let mut context = admission
        .context::<OperationAdmission>()
        .cloned()
        .ok_or_else(|| format!("feed {}'s read isn't an operation", read.feed))?;
    let operation = format!("{}/{}", connector.name, feed.operation);
    if context.admitted.admission.decision != AdmissionDecision::Admit {
        return Err(format!(
            "{operation} would wait on a person, and a feed is read with nobody asked"
        ));
    }
    // A safe read is kept in custody only when its uses are bounded, and a
    // feed's polls are never counted.
    if context.admitted.custody.is_some() {
        return Err(format!(
            "the authority bounds how often {operation} may be used, and a feed's polls aren't counted, so it can't serve feed {}",
            read.feed
        ));
    }
    context.attempt = Some(AttemptPlan {
        number: 1,
        approval: None,
        evidence_refs: Vec::new(),
    });
    let outcome = execute(
        &context,
        &step.id,
        read.env,
        read.workspace,
        &NoTools,
        &mut None,
    )
    .map_err(|error| error.to_string())?;
    let result = outcome
        .value
        .as_object()
        .and_then(|value| value.get("provider_operation"))
        .and_then(JsonValue::as_object)
        .and_then(|packet| packet.get("result"))
        .and_then(JsonValue::as_object)
        .ok_or_else(|| format!("{operation} answered no result"))?;
    page(feed, result).map_err(|why| format!("{operation} answered {why}"))
}

/// The read's inputs: the cursor, and the wait bounded by the most the read
/// declares it takes.
fn inputs(
    feed: &ConnectorFeed,
    manifest: &runx_contracts::connector::ConnectorManifest,
    read: &FeedRead<'_>,
) -> JsonObject {
    let mut inputs = JsonObject::new();
    if let Some(cursor) = read.cursor {
        inputs.insert(feed.cursor.input.as_str().to_owned(), cursor.clone());
    }
    if let (Some(field), Some(wait)) = (&feed.wait, read.wait) {
        let most = manifest
            .operations
            .get(&feed.operation)
            .and_then(|operation| operation.input_schema.as_ref())
            .and_then(|schema| schema.get("properties"))
            .and_then(JsonValue::as_object)
            .and_then(|properties| properties.get(field.as_str()))
            .and_then(JsonValue::as_object)
            .and_then(|property| property.get("maximum"))
            .and_then(JsonValue::as_u64);
        let seconds = most.map_or(wait.as_secs(), |most| wait.as_secs().min(most));
        inputs.insert(field.as_str().to_owned(), JsonValue::Number(seconds.into()));
    }
    inputs
}

/// The page `result` holds, read through the fields `feed` names.
fn page(feed: &ConnectorFeed, result: &JsonObject) -> Result<FeedPage, String> {
    let events = result
        .get(feed.events.result.as_str())
        .and_then(JsonValue::as_array)
        .ok_or_else(|| format!("no list of events in {}", feed.events.result))?
        .iter()
        .map(|event| {
            let id = match event
                .as_object()
                .and_then(|event| event.get(feed.events.id.as_str()))
            {
                Some(JsonValue::String(id)) if !id.is_empty() => id.clone(),
                Some(JsonValue::Number(id)) => id.to_string(),
                _ => return Err(format!("an event with no {}", feed.events.id)),
            };
            Ok(FeedEvent {
                id,
                data: event.clone(),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let flag = |field: Option<&runx_contracts::schema::NonEmptyString>| {
        field.is_some_and(|field| result.get(field.as_str()) == Some(&JsonValue::Bool(true)))
    };
    Ok(FeedPage {
        events,
        cursor: result
            .get(feed.cursor.result.as_str())
            .filter(|cursor| **cursor != JsonValue::Null)
            .cloned(),
        more: flag(feed.more.as_ref()),
        gap: flag(feed.gap.as_ref()),
    })
}

#[cfg(test)]
mod tests;
