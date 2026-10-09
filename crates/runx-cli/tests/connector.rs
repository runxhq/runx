use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Stdio;

use crate::support::{temp_root, unsigned_runx_command_at};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// A `gh` that is signed out.
fn signed_out_gh(bin: &Path) -> TestResult {
    fs::create_dir_all(bin)?;
    let gh = bin.join("gh");
    fs::write(
        &gh,
        "#!/bin/sh\necho 'You are not logged into any GitHub hosts.' >&2\nexit 1\n",
    )?;
    fs::set_permissions(&gh, fs::Permissions::from_mode(0o755))?;
    Ok(())
}

fn runx(
    root: &Path,
    args: &[&str],
    stdin: Option<&str>,
) -> Result<(bool, serde_json::Value), Box<dyn std::error::Error>> {
    let mut child = unsigned_runx_command_at(root)
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", root.join("bin").display()),
        )
        .args(args)
        .arg("--json")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut input = child.stdin.take().ok_or("stdin was not piped")?;
    if let Some(stdin) = stdin {
        input.write_all(stdin.as_bytes())?;
    }
    drop(input);
    let output = child.wait_with_output()?;
    Ok((
        output.status.success(),
        serde_json::from_slice(&output.stdout)
            .map_err(|error| format!("{error}: {}", String::from_utf8_lossy(&output.stderr)))?,
    ))
}

fn connector(
    root: &Path,
    args: &[&str],
) -> Result<(bool, serde_json::Value), Box<dyn std::error::Error>> {
    runx(root, &[&["connector"], args].concat(), None)
}

/// Store a GitHub token as the `operator` profile.
fn stored_token(root: &Path) -> TestResult {
    fs::create_dir_all(root.join(".runx"))?;
    let (ok, set) = runx(
        root,
        &[
            "credential",
            "set",
            "github",
            "--credential",
            "token",
            "--profile",
            "operator",
            "--from-stdin",
        ],
        Some("ghp_connector_test_token"),
    )?;
    assert!(ok, "{set}");
    assert_eq!(
        set["credential"]["instance"]["instance_id"],
        "credential.github.operator"
    );
    Ok(())
}

#[test]
fn an_instance_selects_only_for_its_own_connector() -> TestResult {
    let root = temp_root("runx-connector-select");
    let result = (|| -> TestResult {
        stored_token(&root)?;
        let (ok, used) = connector(&root, &["use", "github", "credential.github.operator"])?;
        assert!(ok, "{used}");
        assert_eq!(
            used["selected"]["instance_id"],
            "credential.github.operator"
        );

        let (ok, listed) = connector(&root, &["instances"])?;
        assert!(ok, "{listed}");
        assert_eq!(listed["instances"][0]["selected"], true);

        // A refused selection records nothing.
        let (ok, refused) = connector(&root, &["use", "slack", "credential.github.operator"])?;
        assert!(!ok);
        assert!(
            refused["error"]["message"]
                .as_str()
                .is_some_and(|message| message.contains("belongs to connector github")),
            "{refused}"
        );
        let (_, listed) = connector(&root, &["list"])?;
        let slack = listed["connectors"]
            .as_array()
            .and_then(|connectors| {
                connectors
                    .iter()
                    .find(|connector| connector["id"] == "slack")
            })
            .ok_or("slack is an official connector")?;
        assert_eq!(slack["selected_instance"], serde_json::Value::Null);
        Ok(())
    })();
    fs::remove_dir_all(&root)?;
    result
}

#[test]
fn inspect_projects_readiness_from_the_selected_instance() -> TestResult {
    let root = temp_root("runx-connector-inspect");
    fs::create_dir_all(&root)?;
    let result = (|| -> TestResult {
        let (ok, before) = connector(&root, &["inspect", "github"])?;
        assert!(ok, "{before}");
        assert!(before["readiness"].as_array().is_some_and(|operations| {
            !operations.is_empty()
                && operations
                    .iter()
                    .all(|operation| operation["ready"] == false)
        }));
        stored_token(&root)?;
        let (ok, _) = connector(&root, &["use", "github", "credential.github.operator"])?;
        assert!(ok);
        let (_, after) = connector(&root, &["inspect", "github"])?;
        assert_eq!(
            after["instances"][0]["instance_id"],
            "credential.github.operator"
        );
        assert!(after["readiness"].as_array().is_some_and(|operations| {
            operations
                .iter()
                .all(|operation| operation["instance_id"] == "credential.github.operator")
        }));
        Ok(())
    })();
    fs::remove_dir_all(&root)?;
    result
}

#[test]
fn a_session_that_prints_no_token_is_refused_and_records_nothing() -> TestResult {
    let root = temp_root("runx-connector-session");
    signed_out_gh(&root.join("bin"))?;
    let result = (|| -> TestResult {
        let (ok, refused) = connector(&root, &["use", "github", "--session"])?;
        assert!(!ok);
        assert!(
            refused["error"]["message"]
                .as_str()
                .is_some_and(|message| message.contains("sign in")),
            "{refused}"
        );
        let (_, listed) = connector(&root, &["instances"])?;
        assert_eq!(listed["instances"], serde_json::json!([]));
        Ok(())
    })();
    fs::remove_dir_all(&root)?;
    result
}

/// Append one event through the data-store skill.
fn append(root: &Path, key: &str) -> Result<(bool, serde_json::Value), Box<dyn std::error::Error>> {
    let skill = crate::support::repo_root()?.join("skills/data-store");
    let key = format!("idempotency_key={key}");
    runx(
        root,
        &[
            "skill",
            skill.to_str().ok_or("the skill path is not text")?,
            "append_event",
            "-i",
            "data_source_ref=local://board",
            "-i",
            "resource=items",
            "-i",
            "aggregate_id=a",
            "--input-json",
            "expected_version=0",
            "-i",
            &key,
            "--input-json",
            r#"event={"type":"item.created"}"#,
        ],
        None,
    )
}

fn status(run: &serde_json::Value) -> &serde_json::Value {
    &run["result"]["data_operation_result"]["data"]["status"]
}

/// Without a store instance the streams live in the workspace store; a
/// confirmed instance's url moves them.
#[test]
fn a_store_instance_moves_the_workspace_streams() -> TestResult {
    let root = temp_root("runx-store-instance");
    fs::create_dir_all(&root)?;
    let result = (|| -> TestResult {
        let (ok, first) = append(&root, "first")?;
        assert!(ok, "{first}");
        assert_eq!(status(&first), "committed");
        assert!(root.join(".runx/store.sqlite").is_file());

        let (ok, used) = connector(
            &root,
            &["use", "store", "--parameter", "url=sqlite:board.sqlite"],
        )?;
        assert!(ok, "{used}");
        assert_eq!(used["selected"]["parameters"]["url"], "sqlite:board.sqlite");

        // The new store is empty, so the stream starts again at version 0.
        let (ok, second) = append(&root, "second")?;
        assert!(ok, "{second}");
        assert_eq!(status(&second), "committed");
        assert!(root.join("board.sqlite").is_file());
        Ok(())
    })();
    fs::remove_dir_all(&root)?;
    result
}

#[test]
fn a_store_url_never_carries_a_password() -> TestResult {
    let root = temp_root("runx-store-password");
    fs::create_dir_all(&root)?;
    let result = (|| -> TestResult {
        let (ok, refused) = connector(
            &root,
            &[
                "use",
                "store",
                "--parameter",
                "url=postgres://app:secret@db.example/board",
            ],
        )?;
        assert!(!ok, "{refused}");
        let (_, listed) = connector(&root, &["instances"])?;
        assert_eq!(listed["instances"], serde_json::json!([]));
        Ok(())
    })();
    fs::remove_dir_all(&root)?;
    result
}

/// A plugin below its workspace root is bound from inside the plugin; the
/// instance lands in the workspace store either way.
#[test]
fn a_plugin_connector_binds_from_inside_its_plugin() -> TestResult {
    let root = temp_root("runx-connector-plugin");
    let plugin = root.join("runx");
    let manifest = plugin.join("connectors/acme");
    fs::create_dir_all(&manifest)?;
    fs::create_dir_all(root.join(".runx"))?;
    fs::write(root.join("pnpm-workspace.yaml"), "packages: []\n")?;
    fs::write(
        manifest.join("connector.yaml"),
        "schema: runx.connector.v1\nid: acme\nname: Acme\ndescription: Read Acme widgets.\n\
         auth:\n  credentials:\n    token:\n      kind: bearer_token\n      fields:\n        - {name: token, label: Acme token}\n\
         \x20     delivery:\n        env: ACME_TOKEN\n      audience: [api.acme.test]\n\
         placements:\n  local:\n    auth: [credentials]\n\
         operations:\n  widgets.read:\n    description: Read the widgets.\n    verb: read\n    scopes: [widgets.read]\n\
         \x20   credentials: [token]\n    target:\n      template: \"https://api.acme.test/widgets\"\n",
    )?;
    let result = (|| -> TestResult {
        let set = [
            "credential",
            "set",
            "acme",
            "--credential",
            "token",
            "--profile",
            "operator",
            "--from-stdin",
        ];
        let (ok, refused) = runx(&root, &set, Some("acme_token"))?;
        assert!(!ok, "the workspace root is not in the plugin: {refused}");

        let (ok, bound) = runx(&plugin, &set, Some("acme_token"))?;
        assert!(ok, "{bound}");
        assert_eq!(
            bound["credential"]["instance"]["instance_id"],
            "credential.acme.operator"
        );
        let (ok, listed) = connector(&root, &["instances"])?;
        assert!(ok, "{listed}");
        assert_eq!(
            listed["instances"][0]["instance_id"],
            "credential.acme.operator"
        );
        let (ok, inspected) = connector(&plugin, &["inspect", "acme"])?;
        assert!(ok, "{inspected}");
        assert_eq!(
            inspected["readiness"][0]["instance_id"], "credential.acme.operator",
            "{inspected}"
        );
        Ok(())
    })();
    fs::remove_dir_all(&root)?;
    result
}
