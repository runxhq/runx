use runx_contracts::{
    ApprovalGate, JsonObject, JsonValue, ResolutionRequest, StandingApproval, StandingCommands,
    StandingEdits, StandingOperations,
};

use super::{Change, covering, covers};
use crate::capability::LocalEffect;
use crate::kernel::operation::OperationWrite;

fn edit(paths: &[&str]) -> Change {
    Change::Local(LocalEffect {
        payload: JsonValue::Null,
        paths: paths.iter().map(|path| (*path).to_owned()).collect(),
        argv: None,
    })
}

fn command(argv: &[&str]) -> Change {
    Change::Local(LocalEffect {
        payload: JsonValue::Null,
        paths: Vec::new(),
        argv: Some(argv.iter().map(|token| (*token).to_owned()).collect()),
    })
}

fn write(connector: &str, operation: &str) -> Change {
    Change::Write {
        write: OperationWrite {
            connector: connector.to_owned(),
            operation: operation.to_owned(),
            approved: false,
            store: false,
        },
        payload: JsonValue::Null,
    }
}

fn edits(paths: &[&str]) -> StandingApproval {
    StandingApproval {
        edits: Some(StandingEdits {
            paths: paths.iter().map(|path| (*path).to_owned()).collect(),
        }),
        ..StandingApproval::default()
    }
}

fn commands(prefix: &[&str]) -> StandingApproval {
    StandingApproval {
        commands: Some(StandingCommands {
            argv_prefix: prefix.iter().map(|token| (*token).into()).collect(),
        }),
        ..StandingApproval::default()
    }
}

fn operations(names: &[&str]) -> StandingApproval {
    StandingApproval {
        operations: Some(StandingOperations {
            names: names.iter().map(|name| (*name).into()).collect(),
        }),
        ..StandingApproval::default()
    }
}

#[test]
fn edits_cover_files_under_their_paths_by_whole_components() {
    let src = [edits(&["src"])];
    assert!(covering(&edit(&["src/lib.rs", "src/a/b.rs"]), &src).is_some());
    assert!(covering(&edit(&["srcx/lib.rs"]), &src).is_none());
    assert!(covering(&edit(&["src/lib.rs", "docs/a.md"]), &src).is_none());
    assert!(covering(&edit(&["anything/at/all"]), &[edits(&[""])]).is_some());
    assert!(
        covering(&command(&["cargo", "test"]), &src).is_none(),
        "edits never cover a process"
    );
}

#[test]
fn commands_cover_an_argv_that_begins_with_their_tokens() {
    let tests = [commands(&["cargo", "test"])];
    assert!(covering(&command(&["cargo", "test"]), &tests).is_some());
    assert!(covering(&command(&["cargo", "test", "-p", "runx"]), &tests).is_some());
    assert!(covering(&command(&["cargo", "testing"]), &tests).is_none());
    assert!(covering(&command(&["cargo"]), &tests).is_none());
    assert!(
        covering(&edit(&["Cargo.toml"]), &tests).is_none(),
        "commands never cover an edit"
    );
}

#[test]
fn local_covers_every_local_effect_and_nothing_covers_without_an_entry() {
    let local = [StandingApproval {
        local: true,
        ..StandingApproval::default()
    }];
    assert!(covering(&edit(&["a.rs"]), &local).is_some());
    assert!(covering(&command(&["rm", "-rf", "build"]), &local).is_some());
    assert!(covering(&edit(&["a.rs"]), &[]).is_none());
    assert!(
        covering(&write("slack", "thread.reply"), &local).is_none(),
        "local never covers a provider write"
    );
}

#[test]
fn operations_cover_a_write_by_its_name_or_its_connector_and_nothing_local() {
    let reply = [operations(&["slack/thread.reply"])];
    assert!(covering(&write("slack", "thread.reply"), &reply).is_some());
    assert!(covering(&write("slack", "channel.post"), &reply).is_none());
    assert!(covering(&write("slackx", "thread.reply"), &reply).is_none());

    let github = [operations(&["github/*"])];
    assert!(covering(&write("github", "comments.create"), &github).is_some());
    assert!(covering(&write("github", "pullrequest.open"), &github).is_some());
    assert!(covering(&write("slack", "thread.reply"), &github).is_none());

    // A nested connector is its own: `google/*` names no operation of
    // `google/search-console`, and the nested one is named in full.
    let google = [operations(&["google/*"])];
    assert!(covering(&write("google/search-console", "sitemaps.submit"), &google).is_none());
    let console = [operations(&["google/search-console/sitemaps.submit"])];
    assert!(covering(&write("google/search-console", "sitemaps.submit"), &console).is_some());

    assert!(
        covering(&edit(&["a.rs"]), &github).is_none(),
        "operations never cover an edit"
    );
    assert!(
        covering(&command(&["git", "push"]), &github).is_none(),
        "operations never cover a process"
    );
}

/// An open approval of the floor, as the run records it.
fn open(gate_type: &str, payload: JsonValue) -> ResolutionRequest {
    ResolutionRequest::Approval {
        id: "run/develop/1/gate".into(),
        gate: Box::new(ApprovalGate {
            id: "develop/1/gate".into(),
            reason: "Approve it".into(),
            gate_type: Some(gate_type.to_owned()),
            summary: None,
            payload: Some(payload),
            payload_artifact: None,
        }),
    }
}

fn object(value: serde_json::Value) -> JsonValue {
    JsonValue::Object(serde_json::from_value::<JsonObject>(value).unwrap_or_default())
}

#[test]
fn new_standing_approvals_answer_only_the_floors_open_gates_they_cover()
-> Result<(), Box<dyn std::error::Error>> {
    let artifacts = tempfile::tempdir()?;
    let reply = [operations(&["slack/thread.reply"])];
    let held = open(
        "write",
        object(serde_json::json!({
            "kind": "operation", "connector": "slack", "operation": "thread.reply", "call": {}
        })),
    );
    assert!(covers(&held, &reply, artifacts.path())?);
    assert!(!covers(
        &held,
        &[operations(&["slack/channel.post"])],
        artifacts.path()
    )?);
    assert!(
        !covers(
            &held,
            &[StandingApproval {
                local: true,
                ..StandingApproval::default()
            }],
            artifacts.path()
        )?,
        "local never answers a held write"
    );

    // An operation's own declared approval is never a standing approval's.
    let declared = open(
        "operation",
        object(serde_json::json!({
            "kind": "operation", "connector": "slack", "operation": "thread.reply"
        })),
    );
    assert!(!covers(&declared, &reply, artifacts.path())?);

    let edit = open(
        "tool",
        object(serde_json::json!({"kind": "edit", "files": [{"path": "src/lib.rs"}]})),
    );
    assert!(covers(&edit, &[edits(&["src"])], artifacts.path())?);
    assert!(!covers(&edit, &reply, artifacts.path())?);
    Ok(())
}

/// Copy `from` into `to`, as a plugin holds a connector.
fn copy_dir(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

#[test]
fn a_write_is_named_with_whether_it_carries_its_own_approval_or_writes_store()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    copy_dir(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/plugins/kernel/connectors/fixture/chat"),
        &root.path().join("connectors/fixture/chat"),
    )?;
    let skill = root.path().join("skill");
    std::fs::create_dir_all(&skill)?;
    let env = std::collections::BTreeMap::from([
        (
            crate::paths::RUNX_CWD_ENV.to_owned(),
            root.path().display().to_string(),
        ),
        (
            crate::paths::RUNX_HOME_ENV.to_owned(),
            root.path().join("home").display().to_string(),
        ),
    ]);
    let target = |requirement: &str, operation: &str| runx_parser::OperationStepTarget {
        requirement: requirement.to_owned(),
        operation: runx_contracts::connector::OperationName::new(operation.to_owned()),
        from: Vec::new(),
        declared: Some(runx_parser::ConnectorRequirement {
            operations: runx_contracts::connector::OperationName::new(operation.to_owned())
                .into_iter()
                .collect(),
            manifest_digest: None,
            choose: None,
            from: Vec::new(),
        }),
        loaded: None,
    };
    let call = |extra: serde_json::Value| -> Result<JsonObject, serde_json::Error> {
        let mut call = serde_json::json!({"idempotency_key": "call-1", "input": {}});
        if let (Some(call), Some(extra)) = (call.as_object_mut(), extra.as_object()) {
            call.extend(extra.clone());
        }
        serde_json::from_value(call)
    };
    let write = |requirement: &str, operation: &str, extra: serde_json::Value| {
        Ok::<_, serde_json::Error>(crate::kernel::operation::operation_write(
            &target(requirement, operation),
            &call(extra)?,
            &env,
            &skill,
        ))
    };
    let named = |connector: &str, operation: &str, approved: bool, store: bool| {
        Some(OperationWrite {
            connector: connector.to_owned(),
            operation: operation.to_owned(),
            approved,
            store,
        })
    };

    // A write with no approval of its own is what `ask` holds.
    assert_eq!(
        write("fixture/chat", "thread.reply", serde_json::json!({}))?,
        named("fixture/chat", "thread.reply", false, false)
    );
    // An operation's declared approval, or a step's, is its own.
    assert_eq!(
        write("fixture/chat", "channel.post", serde_json::json!({}))?,
        named("fixture/chat", "channel.post", true, false)
    );
    assert_eq!(
        write(
            "fixture/chat",
            "thread.reply",
            serde_json::json!({"approval": {"reason": "Approve this reply."}})
        )?,
        named("fixture/chat", "thread.reply", true, false)
    );
    // A read is no write.
    assert_eq!(
        write("fixture/chat", "thread.read", serde_json::json!({}))?,
        None
    );
    // A store append writes store's streams.
    assert_eq!(
        write("store", "events.append", serde_json::json!({}))?,
        named("store", "events.append", false, true)
    );
    Ok(())
}
