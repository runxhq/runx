use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Stdio;
use std::thread;

use crate::support::{plugin_with_source_connectors, temp_root, unsigned_runx_command_at};

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

/// Store a GitHub token as the `operator` profile, and return the instance
/// it confirms.
fn stored_token(root: &Path) -> Result<String, Box<dyn std::error::Error>> {
    fs::create_dir_all(root.join(".runx"))?;
    let (ok, set) = runx(
        root,
        &[
            "connector",
            "use",
            "github",
            "--credential",
            "token",
            "--profile",
            "operator",
        ],
        Some("ghp_connector_test_token"),
    )?;
    assert!(ok, "{set}");
    let instance = set["selected"]["instance_id"]
        .as_str()
        .ok_or("the credential confirmed no instance")?;
    assert!(instance.starts_with("credential."), "{set}");
    Ok(instance.to_owned())
}

#[test]
fn an_instance_selects_only_for_its_own_connector() -> TestResult {
    let root = temp_root("runx-connector-select");
    plugin_with_source_connectors(&root, &["github", "slack"])?;
    let result = (|| -> TestResult {
        let instance = stored_token(&root)?;
        let (ok, used) = connector(&root, &["use", "github", &instance])?;
        assert!(ok, "{used}");
        assert_eq!(used["selected"]["instance_id"], instance.as_str());

        let (ok, listed) = connector(&root, &["list"])?;
        assert!(ok, "{listed}");
        assert_eq!(listed["instances"][0]["selected"], true);

        // A refused selection records nothing.
        let (ok, refused) = connector(&root, &["use", "slack", &instance])?;
        assert!(!ok);
        assert!(
            refused["error"]["message"]
                .as_str()
                .is_some_and(|message| message.contains("belongs to connector runx/github")),
            "{refused}"
        );
        let (_, listed) = connector(&root, &["list"])?;
        let slack = listed["connectors"]
            .as_array()
            .and_then(|connectors| {
                connectors
                    .iter()
                    .find(|connector| connector["connector_binding"]["connector"] == "runx/slack")
            })
            .ok_or("runx/slack is listed")?;
        assert_eq!(slack["selected_instance"], serde_json::Value::Null);
        Ok(())
    })();
    fs::remove_dir_all(&root)?;
    result
}

#[test]
fn inspect_projects_readiness_from_the_selected_instance() -> TestResult {
    let root = temp_root("runx-connector-inspect");
    plugin_with_source_connectors(&root, &["github"])?;
    let result = (|| -> TestResult {
        let (ok, before) = connector(&root, &["inspect", "github"])?;
        assert!(ok, "{before}");
        assert!(before["readiness"].as_array().is_some_and(|operations| {
            !operations.is_empty()
                && operations
                    .iter()
                    .all(|operation| operation["ready"] == false)
        }));
        let instance = stored_token(&root)?;
        let (ok, _) = connector(&root, &["use", "github", &instance])?;
        assert!(ok);
        let (_, after) = connector(&root, &["inspect", "github"])?;
        assert_eq!(after["instances"][0]["instance_id"], instance.as_str());
        assert!(after["readiness"].as_array().is_some_and(|operations| {
            operations
                .iter()
                .all(|operation| operation["instance_id"] == instance.as_str())
        }));
        Ok(())
    })();
    fs::remove_dir_all(&root)?;
    result
}

#[test]
fn a_session_that_prints_no_token_is_refused_and_records_nothing() -> TestResult {
    let root = temp_root("runx-connector-session");
    plugin_with_source_connectors(&root, &["github"])?;
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
        let (_, listed) = connector(&root, &["list"])?;
        assert_eq!(listed["instances"], serde_json::json!([]));

        // Read by a person, the refusal still fails the command.
        let plain = unsigned_runx_command_at(&root)
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", root.join("bin").display()),
            )
            .args(["connector", "use", "github", "--session"])
            .output()?;
        assert_eq!(plain.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&plain.stderr).contains("sign in"));
        Ok(())
    })();
    fs::remove_dir_all(&root)?;
    result
}

/// A profile name holds one connector's credential: binding another
/// connector's under it is refused, so the instance pinned to it keeps working.
#[test]
fn a_profile_name_is_not_rebound_to_another_connector() -> TestResult {
    let root = temp_root("runx-connector-profile-held");
    plugin_with_source_connectors(&root, &["github", "linear"])?;
    let result = (|| -> TestResult {
        let (ok, github) = runx(
            &root,
            &[
                "connector",
                "use",
                "github",
                "--credential",
                "token",
                "--profile",
                "shared",
            ],
            Some("ghp_connector_test_token"),
        )?;
        assert!(ok, "{github}");
        let (ok, refused) = runx(
            &root,
            &[
                "connector",
                "use",
                "linear",
                "--credential",
                "api_key",
                "--profile",
                "shared",
            ],
            Some("lin_connector_test_key"),
        )?;
        assert!(!ok, "{refused}");
        assert!(
            refused["error"]["message"]
                .as_str()
                .is_some_and(|message| message
                    .contains("credential profile shared holds runx/github's token")),
            "{refused}"
        );
        let (_, inspected) = connector(&root, &["inspect", "github"])?;
        assert!(
            inspected["readiness"]
                .as_array()
                .is_some_and(|operations| operations
                    .iter()
                    .any(|operation| operation["ready"] == true)),
            "{inspected}"
        );
        Ok(())
    })();
    fs::remove_dir_all(&root)?;
    result
}

#[test]
fn inspect_without_an_instance_says_how_to_set_one_up_once() -> TestResult {
    let root = temp_root("runx-connector-inspect-setup");
    plugin_with_source_connectors(&root, &["github"])?;
    let result = (|| -> TestResult {
        let output = unsigned_runx_command_at(&root)
            .args(["connector", "inspect", "github"])
            .output()?;
        assert!(output.status.success());
        let text = String::from_utf8(output.stdout)?;
        assert!(text.contains("instances: none\n"), "{text}");
        assert!(text.contains("  set up: runx connector use "), "{text}");
        assert!(!text.contains("shows how to set one up"), "{text}");
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
            "-i",
            "expected_version=0",
            "-i",
            &key,
            "-i",
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
        let (_, listed) = connector(&root, &["list"])?;
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
    fs::write(plugin.join("plugin.json"), r#"{"namespace":"acme"}"#)?;
    fs::write(
        manifest.join("connector.yaml"),
        "schema: runx.connector.v1\nid: acme\nname: Acme\ndescription: Read Acme widgets.\nruntime_capability: false\n\
         auth:\n  credentials:\n    token:\n      kind: bearer_token\n      fields:\n        - {name: token, label: Acme token}\n\
         \x20     delivery:\n        env: ACME_TOKEN\n      audience: [api.acme.test]\n\
         placements:\n  local:\n    auth: [credentials]\n\
         operations:\n  widgets.read:\n    description: Read the widgets.\n    verb: read\n    scopes: [widgets.read]\n\
         \x20   credentials: [token]\n    target:\n      template: \"https://api.acme.test/widgets\"\n",
    )?;
    let result = (|| -> TestResult {
        let set = [
            "connector",
            "use",
            "acme",
            "--credential",
            "token",
            "--profile",
            "operator",
        ];
        let (ok, refused) = runx(&root, &set, Some("acme_token"))?;
        assert!(!ok, "the workspace root is not in the plugin: {refused}");
        let canonical = |path: &Path| path.canonicalize().map(|path| path.display().to_string());
        let message = refused.to_string();
        assert!(
            message.contains(&format!("{} is in no plugin", canonical(&root)?))
                && message.contains("reached from inside the plugin"),
            "a miss says where it looked: {message}"
        );

        let (ok, bound) = runx(&plugin, &set, Some("acme_token"))?;
        assert!(ok, "{bound}");
        let instance = &bound["selected"];
        assert_eq!(instance["connector"], "acme/acme", "{bound}");
        let (ok, listed) = connector(&root, &["list"])?;
        assert!(ok, "{listed}");
        assert_eq!(
            listed["instances"][0]["instance_id"],
            instance["instance_id"]
        );
        let (ok, inspected) = connector(&plugin, &["inspect", "acme"])?;
        assert!(ok, "{inspected}");
        assert_eq!(
            inspected["readiness"][0]["instance_id"], instance["instance_id"],
            "{inspected}"
        );
        let (ok, missing) = connector(&plugin, &["inspect", "acme-two"])?;
        assert!(!ok, "{missing}");
        assert!(
            missing.to_string().contains(&format!(
                "connector acme-two is neither a runtime capability nor one of the plugin at {}",
                canonical(&plugin)?
            )),
            "{missing}"
        );
        Ok(())
    })();
    fs::remove_dir_all(&root)?;
    result
}

/// `connector list` shows the signed-in account's runx Connect grants beside
/// the connectors in reach.
#[test]
fn connector_list_reads_the_signed_in_accounts_grants() -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    let server = thread::spawn(move || -> Result<(), String> {
        for (path, body) in [
            (
                "/v1/me",
                r#"{"status":"success","principal":{"principal_id":"fixture-user","role":"user"}}"#,
            ),
            ("/v1/grants", r#"{"status":"success","grants":[]}"#),
        ] {
            let (mut stream, _) = listener.accept().map_err(|error| error.to_string())?;
            let mut request = [0u8; 8192];
            let length = stream
                .read(&mut request)
                .map_err(|error| error.to_string())?;
            let request = String::from_utf8_lossy(&request[..length]);
            if !request.starts_with(&format!("GET {path} HTTP/1.1")) {
                return Err(format!("unexpected request for {path}: {request}"));
            }
            if !request
                .to_ascii_lowercase()
                .contains("authorization: bearer rxk_fixture")
            {
                return Err(format!("request for {path} omitted the bearer token"));
            }
            write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .map_err(|error| error.to_string())?;
        }
        Ok(())
    });

    let output = crate::support::isolated_runx_command_with_inherited_cwd("connector-list-test")
        .env("RUNX_PUBLIC_API_TOKEN", "rxk_fixture")
        .env("RUNX_PUBLIC_API_BASE_URL", format!("http://{address}"))
        .env("RUNX_PUBLIC_API_ALLOW_PRIVATE_NETWORK", "1")
        .args(["connector", "list", "--json"])
        .output()?;
    let server_result = server
        .join()
        .map_err(|_| "Connect fixture server panicked")?;
    server_result.map_err(|error| -> Box<dyn std::error::Error> { error.into() })?;

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(body["grants"], serde_json::json!([]));
    assert!(
        body["connectors"]
            .as_array()
            .is_some_and(|connectors| !connectors.is_empty())
    );
    Ok(())
}
