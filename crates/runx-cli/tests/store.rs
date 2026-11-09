//! Who may touch a stream (plan §4.8, `store.md`), as the CLI runs it: a
//! package reads and writes its own `store` streams with no naming, another
//! package reads one only under a term and never writes it, every event
//! records its writer, a run's `packages` bounds every call's chain, and every
//! `private` call needs a term.
//!
//! Each case runs `runx skill` in a scratch project with its own runx home.
//! A registry package is published to a loopback registry that signs it, and
//! installed from there, as a published package is.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{Value, json};

use crate::registry_fixture::{LoopbackRegistry, Publication};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// A package that keeps a count per key in its own stream, `counts`: `bump`
/// inspects the key and appends at the version it read, `read` reads the
/// key's events, `inspect` reads its newest event and `list` its keys.
const COUNTER: &str = r#"minimum_runx: 0.12.0
methods:
  bump:
    inputs:
      key: {type: string}
      attempt: {type: string}
    steps:
      - id: before
        operation: store/key.inspect
        inputs:
          stream: counts
          key: $input.key
      - id: append
        operation: store/events.append
        idempotency_key: $input.attempt
        inputs:
          stream: counts
          key: $input.key
          expected_version: $steps.before.provider_operation.result.version
          type: counter.bumped
          data:
            attempt: $input.attempt
  read:
    inputs:
      key: {type: string}
    steps:
      - id: events
        operation: store/events.read
        inputs:
          stream: counts
          key: $input.key
  inspect:
    inputs:
      key: {type: string}
    steps:
      - id: key
        operation: store/key.inspect
        inputs:
          stream: counts
          key: $input.key
  list:
    steps:
      - id: keys
        operation: store/keys.list
        inputs:
          stream: counts
"#;

fn manual(name: &str) -> String {
    format!("---\nname: {name}\ndescription: A {name} fixture.\n---\n# {name}\n")
}

/// A package that composes `reference`'s `bump` and `read`.
fn wrapper(reference: &str) -> String {
    format!(
        r#"minimum_runx: 0.12.0
methods:
  bump:
    inputs:
      key: {{type: string}}
      attempt: {{type: string}}
    steps:
      - id: bump
        skill: {reference}#bump
        inputs:
          key: $input.key
          attempt: $input.attempt
  read:
    inputs:
      key: {{type: string}}
    steps:
      - id: read
        skill: {reference}#read
        inputs:
          key: $input.key
"#
    )
}

/// One append whose every field is the caller's, for the refusals: the
/// stream, the key and any field beside them.
fn appender(inputs: &str) -> String {
    format!(
        r#"minimum_runx: 0.12.0
methods:
  append:
    inputs:
      attempt: {{type: string}}
    steps:
      - id: append
        operation: store/events.append
        idempotency_key: $input.attempt
        inputs:
{inputs}
"#
    )
}

/// A scratch project with its own runx home.
struct Project {
    root: PathBuf,
    registry: Option<LoopbackRegistry>,
}

impl Project {
    fn new(prefix: &str) -> TestResult<Self> {
        let root = crate::support::temp_root(prefix);
        std::fs::create_dir_all(root.join("home"))?;
        Ok(Self {
            root,
            registry: None,
        })
    }

    /// A project that installs from a loopback registry trusting `acme`.
    fn with_registry(prefix: &str) -> TestResult<Self> {
        let mut project = Self::new(prefix)?;
        project.registry = Some(LoopbackRegistry::serve()?);
        Ok(project)
    }

    fn registry(&self) -> TestResult<&LoopbackRegistry> {
        self.registry
            .as_ref()
            .ok_or_else(|| "the project has no registry".into())
    }

    fn write(&self, path: &str, contents: &str) -> TestResult<PathBuf> {
        let path = self.root.join(path);
        std::fs::create_dir_all(path.parent().ok_or("a file has a directory")?)?;
        std::fs::write(&path, contents)?;
        Ok(path)
    }

    /// A project skill at `skills/<name>`.
    fn skill(&self, name: &str, profile: &str) -> TestResult<PathBuf> {
        self.write(&format!("skills/{name}/SKILL.md"), &manual(name))?;
        self.write(&format!("skills/{name}/run.yaml"), profile)?;
        Ok(self.root.join("skills").join(name))
    }

    fn runx(&self) -> TestResult<Command> {
        let mut command = crate::support::unsigned_runx_command_at(&self.root);
        command.env("RUNX_CWD", &self.root);
        if let Some(registry) = &self.registry {
            command
                .env("RUNX_REGISTRY_URL", &registry.url)
                .envs(registry.env()?);
        }
        Ok(command)
    }

    /// Publish `files` as `<publisher>/<name>@1.0.0`, signed, and install it.
    fn publish(
        &self,
        publisher: &str,
        name: &str,
        profile: &str,
        files: &[(&str, &str)],
    ) -> TestResult {
        let registry = self.registry()?;
        let markdown = manual(name);
        let mut publication = Publication::new(publisher, "1.0.0", &markdown, Some(profile));
        publication.files = files
            .iter()
            .map(
                |(path, content)| runx_runtime::registry::RegistryPackageFile {
                    path: (*path).to_owned(),
                    content: (*content).to_owned(),
                },
            )
            .collect();
        registry.publish(publication)?;
        let installed = self
            .runx()?
            .args(["registry", "install", &format!("{publisher}/{name}@1.0.0")])
            .args(["--json"])
            .output()?;
        json(&installed, 0)?;
        Ok(())
    }

    /// `runx skill <reference> -i ...` under `authority`, and its JSON.
    fn run(
        &self,
        reference: &str,
        inputs: &[(&str, &str)],
        authority: Option<&Value>,
    ) -> TestResult<Output> {
        let mut command = self.runx()?;
        command.args(["skill", reference]);
        for (name, value) in inputs {
            command.args(["-i", &format!("{name}={value}")]);
        }
        if let Some(authority) = authority {
            command.args(["--run-authority", &authority.to_string()]);
        }
        Ok(command.arg("--json").output()?)
    }

    /// The store result a run of `reference` ended with.
    fn result(
        &self,
        reference: &str,
        inputs: &[(&str, &str)],
        authority: Option<&Value>,
    ) -> TestResult<Value> {
        let ran = json(&self.run(reference, inputs, authority)?, 0)?;
        assert_eq!(ran["outcome"], "completed", "{ran}");
        Ok(store_result(&ran))
    }
}

/// `output`'s JSON, after its exit is `exit`.
fn json(output: &Output, exit: i32) -> TestResult<Value> {
    assert_eq!(
        output.status.code(),
        Some(exit),
        "stderr={}\nstdout={}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    Ok(serde_json::from_slice(&output.stdout)?)
}

/// What a run printed, as text, for a refusal's reason.
fn printed(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// The store result a run ended with.
fn store_result(ran: &Value) -> Value {
    ran["result"]["provider_operation"]["data"]["result"].clone()
}

/// A run's authority with one delegation on `connector`'s instance, holding
/// one term of `verbs` over `targets` on `operations`, for every package. A
/// plain narrowing: it names no actor, mandate or principal.
fn delegated(
    connector: &str,
    verbs: &[&str],
    operations: &[&str],
    scopes: &[&str],
    targets: &[&str],
) -> Value {
    json!({
        "delegations": [{
            "connector": connector,
            "terms": [{
                "verbs": verbs,
                "bounds": {
                    "operations": operations,
                    "scopes": scopes,
                    "targets": targets,
                    "packages": ["*/*", "project:*"],
                },
            }],
        }],
    })
}

// A package reads and writes its own streams with no naming.

#[test]
fn a_package_reads_and_writes_its_own_stream_with_no_naming() -> TestResult {
    let project = Project::new("runx-store-own")?;
    project.skill("counter", COUNTER)?;

    let first = project.result(
        "skills/counter#bump",
        &[("key", "k"), ("attempt", "one")],
        None,
    )?;
    assert_eq!(first["schema"], "runx.store.result.v1", "{first}");
    assert_eq!(first["operation"], "events.append");
    assert_eq!(first["stream"], "project:counter/counts");
    assert_eq!(first["key"], "k");
    assert_eq!(first["backend"], "sqlite");
    assert_eq!(first["status"], "committed");
    assert_eq!(
        (first["before_version"].as_u64(), first["version"].as_u64()),
        (Some(0), Some(1))
    );
    assert!(
        first["event"]["id"].is_string() && first["event"]["digest"].is_string(),
        "{first}"
    );
    project.result(
        "skills/counter#bump",
        &[("key", "k"), ("attempt", "two")],
        None,
    )?;

    let read = project.result("skills/counter#read", &[("key", "k")], None)?;
    assert_eq!(read["stream"], "project:counter/counts");
    assert_eq!(read["version"], 2);
    assert_eq!(read["last_version"], 2);
    assert_eq!(read["has_more"], false);
    let events = read["events"].as_array().ok_or("events is a list")?;
    assert_eq!(events.len(), 2, "{read}");
    for (index, event) in events.iter().enumerate() {
        assert_eq!(event["version"], index + 1);
        assert_eq!(event["type"], "counter.bumped");
        // The runtime records who wrote it, never the caller.
        assert_eq!(event["writer"]["package"], "project:counter", "{event}");
        assert!(
            event["writer"]["digest"]
                .as_str()
                .is_some_and(|digest| digest.starts_with("sha256:")),
            "{event}"
        );
    }
    assert_eq!(events[0]["data"], json!({"attempt": "one"}));

    let inspected = project.result("skills/counter#inspect", &[("key", "k")], None)?;
    assert_eq!(inspected["operation"], "key.inspect");
    assert_eq!(inspected["version"], 2);
    assert_eq!(
        inspected["last_event"], events[1],
        "the newest event, whole"
    );
    assert_eq!(inspected["chain_digest"], read["chain_digest"]);

    let listed = project.result("skills/counter#list", &[], None)?;
    assert_eq!(listed["operation"], "keys.list");
    assert_eq!(
        listed["keys"],
        json!([{"key": "k", "last_event": events[1]}])
    );
    assert_eq!(listed["next_cursor"], Value::Null);
    Ok(())
}

// A package can't write another's stream, whatever it passes.

#[test]
fn a_write_never_names_a_package_and_a_stream_never_spells_one() -> TestResult {
    let project = Project::new("runx-store-spelled")?;
    project.skill("reader", COUNTER)?;
    project.result(
        "skills/reader#bump",
        &[("key", "k"), ("attempt", "own")],
        None,
    )?;
    for (name, inputs) in [
        // A write has no package field.
        (
            "named",
            "          package: project:reader\n          stream: counts\n          key: k\n          expected_version: 1\n          type: counter.bumped\n          data: {}",
        ),
        // A stream is one segment, so it can't spell another package's.
        (
            "slashed",
            "          stream: project:reader/counts\n          key: k\n          expected_version: 1\n          type: counter.bumped\n          data: {}",
        ),
        (
            "climbing",
            "          stream: ../reader/counts\n          key: k\n          expected_version: 1\n          type: counter.bumped\n          data: {}",
        ),
        // Today's addressing is gone.
        (
            "addressed",
            "          data_source_uri: local://reader\n          resource: counts\n          aggregate_id: k\n          expected_version: 1\n          event: {type: counter.bumped}",
        ),
    ] {
        project.skill(name, &appender(inputs))?;
        let refused = project.run(&format!("skills/{name}#append"), &[("attempt", name)], None)?;
        assert!(
            !refused.status.success(),
            "{name} appended: {}",
            printed(&refused)
        );
    }
    let read = project.result("skills/reader#read", &[("key", "k")], None)?;
    assert_eq!(read["events"].as_array().map(Vec::len), Some(1), "{read}");
    Ok(())
}

/// A project package whose manifest says it's runx's is the person's own: its
/// streams are `project:reddit/...`, never `runx/reddit/...`.
#[test]
fn a_project_package_named_as_runxs_writes_only_its_own_streams() -> TestResult {
    let project = Project::new("runx-store-claims-runx")?;
    // It calls itself reddit, as runx's reddit does, and ships an install
    // record claiming runx/reddit, first party.
    project.skill("reddit", COUNTER)?;
    project.write(
        ".runx/installs/added-skills/runx/reddit/1.0.0.json",
        r#"{"package": "runx/reddit@1.0.0", "trust_tier": "first_party"}"#,
    )?;
    let appended = project.result(
        "skills/reddit#bump",
        &[("key", "alice"), ("attempt", "claim")],
        None,
    )?;
    assert_eq!(appended["stream"], "project:reddit/counts", "{appended}");
    let read = project.result("skills/reddit#read", &[("key", "alice")], None)?;
    assert_eq!(read["events"][0]["writer"]["package"], "project:reddit");
    Ok(())
}

// A stateful package dependency reads and writes the same streams standalone
// and through two wrappers.

#[test]
fn a_package_dependency_keeps_its_streams_standalone_and_through_two_wrappers() -> TestResult {
    let project = Project::with_registry("runx-store-dependency")?;
    project.publish("acme", "counter", COUNTER, &[])?;
    let dependency = "registry:acme/counter@1.0.0";
    project.skill("wrap-a", &wrapper(dependency))?;
    project.skill("wrap-b", &wrapper(dependency))?;

    let standalone = project.result(
        "acme/counter@1.0.0#bump",
        &[("key", "k"), ("attempt", "standalone")],
        None,
    )?;
    assert_eq!(standalone["stream"], "acme/counter/counts", "{standalone}");
    json(
        &project.run(
            "skills/wrap-a#bump",
            &[("key", "k"), ("attempt", "wrap-a")],
            None,
        )?,
        0,
    )?;
    json(
        &project.run(
            "skills/wrap-b#bump",
            &[("key", "k"), ("attempt", "wrap-b")],
            None,
        )?,
        0,
    )?;

    for reader in [
        "acme/counter@1.0.0#read",
        "skills/wrap-a#read",
        "skills/wrap-b#read",
    ] {
        let ran = json(&project.run(reader, &[("key", "k")], None)?, 0)?;
        let read = find_store_result(&ran, "events.read")
            .ok_or_else(|| format!("{reader} read nothing: {ran}"))?;
        assert_eq!(read["stream"], "acme/counter/counts", "{reader}: {read}");
        let events = read["events"].as_array().ok_or("events is a list")?;
        assert_eq!(
            events
                .iter()
                .map(|event| event["data"]["attempt"].clone())
                .collect::<Vec<_>>(),
            vec![json!("standalone"), json!("wrap-a"), json!("wrap-b")],
            "{reader}"
        );
        // One package wrote all three, from the same bytes.
        assert!(
            events
                .iter()
                .all(|event| event["writer"]["package"] == "acme/counter"),
            "{read}"
        );
        assert!(
            events
                .iter()
                .all(|event| event["writer"]["digest"] == events[0]["writer"]["digest"]),
            "{read}"
        );
    }
    Ok(())
}

/// The store result for `operation` anywhere in what a run printed: a
/// wrapper's result holds its child's.
fn find_store_result(value: &Value, operation: &str) -> Option<Value> {
    match value {
        Value::Object(object) => {
            if object.get("schema") == Some(&json!("runx.store.result.v1"))
                && object.get("operation") == Some(&json!(operation))
            {
                return Some(value.clone());
            }
            object
                .values()
                .find_map(|child| find_store_result(child, operation))
        }
        Value::Array(items) => items
            .iter()
            .find_map(|child| find_store_result(child, operation)),
        _ => None,
    }
}

/// A community package's incorporated dependency named `github` writes under
/// the package carrying it, and never meets its publisher's real `github`.
#[test]
fn an_incorporated_github_writes_under_its_carrier_and_never_meets_the_real_one() -> TestResult {
    let project = Project::with_registry("runx-store-incorporated")?;
    let positions = r#"minimum_runx: 0.12.0
methods:
  mark:
    inputs:
      attempt: {type: string}
    steps:
      - id: before
        operation: store/key.inspect
        inputs: {stream: positions, key: main}
      - id: append
        operation: store/events.append
        idempotency_key: $input.attempt
        inputs:
          stream: positions
          key: main
          expected_version: $steps.before.provider_operation.result.version
          type: github.position
          data: {attempt: $input.attempt}
  read:
    steps:
      - id: read
        operation: store/events.read
        inputs: {stream: positions, key: main}
"#;
    let carrier = r#"minimum_runx: 0.12.0
methods:
  mark:
    inputs:
      attempt: {type: string}
    steps:
      - id: mark
        skill: ./github#mark
        inputs: {attempt: $input.attempt}
  read:
    steps:
      - id: read
        skill: ./github#read
"#;
    let github_manual = manual("github");
    project.publish(
        "acme",
        "foo",
        carrier,
        &[
            ("github/SKILL.md", &github_manual),
            ("github/run.yaml", positions),
        ],
    )?;
    project.publish("acme", "github", positions, &[])?;

    let carried = json(
        &project.run("acme/foo@1.0.0#mark", &[("attempt", "carried")], None)?,
        0,
    )?;
    let carried =
        find_store_result(&carried, "events.append").ok_or("the carrier appended nothing")?;
    assert_eq!(carried["stream"], "acme/foo/github/positions", "{carried}");
    let real = project.result("acme/github@1.0.0#mark", &[("attempt", "real")], None)?;
    assert_eq!(real["stream"], "acme/github/positions", "{real}");
    // Each begins its own key at version 1: neither saw the other's event.
    assert_eq!(carried["version"], 1);
    assert_eq!(real["version"], 1);

    let carried_read = json(&project.run("acme/foo@1.0.0#read", &[], None)?, 0)?;
    let carried_read =
        find_store_result(&carried_read, "events.read").ok_or("the carrier read nothing")?;
    assert_eq!(carried_read["events"].as_array().map(Vec::len), Some(1));
    // Incorporated code is its carrier's.
    assert_eq!(carried_read["events"][0]["writer"]["package"], "acme/foo");
    let real_read = project.result("acme/github@1.0.0#read", &[], None)?;
    assert_eq!(real_read["events"][0]["writer"]["package"], "acme/github");
    assert_eq!(real_read["events"].as_array().map(Vec::len), Some(1));
    Ok(())
}

// Another package reads a stream only under a term, and never writes it.

/// A package that reads `counter`'s `counts` by naming it.
const NAMED_READER: &str = r#"minimum_runx: 0.12.0
methods:
  read:
    inputs:
      key: {type: string}
    steps:
      - id: events
        operation: store/events.read
        inputs:
          package: project:counter
          stream: counts
          key: $input.key
"#;

#[test]
fn another_packages_stream_is_read_only_under_a_term() -> TestResult {
    let project = Project::new("runx-store-named-read")?;
    project.skill("counter", COUNTER)?;
    project.skill("observer", NAMED_READER)?;
    project.result(
        "skills/counter#bump",
        &[("key", "k"), ("attempt", "one")],
        None,
    )?;

    let unnamed = project.run("skills/observer#read", &[("key", "k")], None)?;
    assert!(!unnamed.status.success(), "{}", printed(&unnamed));
    assert!(
        printed(&unnamed).contains("project:counter"),
        "{}",
        printed(&unnamed)
    );

    let term = delegated(
        "store",
        &["read"],
        &["store/events.read"],
        &["store.read"],
        &["project:counter/counts"],
    );
    let read = project.result("skills/observer#read", &[("key", "k")], Some(&term))?;
    assert_eq!(read["stream"], "project:counter/counts");
    assert_eq!(read["events"].as_array().map(Vec::len), Some(1));

    // A term naming another stream doesn't reach this one.
    let elsewhere = delegated(
        "store",
        &["read"],
        &["store/events.read"],
        &["store.read"],
        &["project:other/*"],
    );
    let refused = project.run("skills/observer#read", &[("key", "k")], Some(&elsewhere))?;
    assert!(!refused.status.success(), "{}", printed(&refused));
    Ok(())
}

/// operations-review reads another package's stream before it plans, and
/// only under a term naming that stream (plan §4.8, the rule's first test).
#[test]
fn operations_review_reads_another_packages_stream_under_a_term() -> TestResult {
    let project = Project::new("runx-store-operations-review")?;
    project.skill("counter", COUNTER)?;
    project.result(
        "skills/counter#bump",
        &[("key", "k"), ("attempt", "one")],
        None,
    )?;
    let review = crate::support::repo_root()?.join("skills/operations-review");
    let reference = format!("{}#operate_from_projection", review.display());
    let inputs = [
        ("package", "project:counter"),
        ("stream", "counts"),
        ("key", "k"),
        ("objective", "Tell me the next safe action."),
        ("scope", "workspace:example"),
    ];
    let unnamed = project.run(&reference, &inputs, None)?;
    assert_eq!(unnamed.status.code(), Some(1), "{}", printed(&unnamed));
    assert!(
        printed(&unnamed).contains("another package's stream"),
        "{}",
        printed(&unnamed)
    );

    let term = delegated(
        "store",
        &["read"],
        &["store/key.inspect"],
        &["store.read"],
        &["project:counter/counts"],
    );
    // The read passes, and the review waits on its agent with the key's
    // state as its snapshot.
    let reviewed = json(&project.run(&reference, &inputs, Some(&term))?, 2)?;
    assert_eq!(reviewed["status"], "needs_agent", "{reviewed}");
    let request = std::fs::read_to_string(
        reviewed["requests"][0]["artifact"]["path"]
            .as_str()
            .ok_or("the turn's request names its artifact")?,
    )?;
    assert!(request.contains("project:counter/counts"), "{request}");
    Ok(())
}

// `packages`: every package in a call's chain must be covered (authority.md,
// the 0state-a3 probe's table).

/// A project parent, `work`, composing acme's counter, which writes.
fn composed(prefix: &str) -> TestResult<Project> {
    let project = Project::with_registry(prefix)?;
    project.publish("acme", "counter", COUNTER, &[])?;
    project.skill("work", &wrapper("registry:acme/counter@1.0.0"))?;
    Ok(project)
}

fn packages(names: &[&str]) -> Value {
    json!({"packages": names})
}

#[test]
fn a_runs_packages_bound_every_calls_chain() -> TestResult {
    let project = composed("runx-store-packages")?;
    let bump = |attempt: &str, authority: Option<&Value>| {
        project.run(
            "skills/work#bump",
            &[("key", "k"), ("attempt", attempt)],
            authority,
        )
    };
    // No packages: today's behaviour.
    json(&bump("unbounded", None)?, 0)?;

    // The parent alone: the child's call is refused before any attempt.
    let parent_only = bump("parent-only", Some(&packages(&["project:work"])))?;
    assert!(!parent_only.status.success(), "{}", printed(&parent_only));
    assert!(
        printed(&parent_only).contains("acme/counter"),
        "{}",
        printed(&parent_only)
    );

    // The parent and the child, by name or by pattern: admitted, and the
    // execution term names the chain.
    for (attempt, bound) in [
        ("both", packages(&["project:work", "acme/counter"])),
        ("pattern", packages(&["project:work", "acme/*"])),
    ] {
        let ran = json(&bump(attempt, Some(&bound))?, 0)?;
        let appended = find_store_result(&ran, "events.append").ok_or("nothing appended")?;
        assert_eq!(appended["status"], "committed", "{appended}");
        let receipt = project.receipt_with_term(&ran, "store/events.append")?;
        assert_eq!(
            receipt["bounds"]["packages"],
            json!(["acme/counter", "project:work"]),
            "{receipt}"
        );
    }

    // An empty list covers nothing, the parent's own calls included.
    project.skill("own", COUNTER)?;
    let none = project.run(
        "skills/own#bump",
        &[("key", "k"), ("attempt", "none")],
        Some(&packages(&[])),
    )?;
    assert!(!none.status.success(), "{}", printed(&none));

    // Readiness says the same, through the same function.
    let inspected = json(
        &project
            .runx()?
            .args(["skill", "inspect", "skills/own#bump"])
            .args([
                "--run-authority",
                &packages(&["project:elsewhere"]).to_string(),
            ])
            .arg("--json")
            .output()?,
        0,
    )?;
    let readiness = inspected["connector_readiness"]
        .as_array()
        .ok_or("readiness is a list")?;
    assert!(
        readiness.iter().all(|entry| entry["ready"] == false),
        "{inspected}"
    );
    Ok(())
}

impl Project {
    /// The execution term of the call to `operation` a run's receipts hold.
    fn receipt_with_term(&self, ran: &Value, operation: &str) -> TestResult<Value> {
        let receipts = self.root.join(".runx/receipts");
        let mut found = None;
        for entry in std::fs::read_dir(&receipts)? {
            let receipt: Value = match serde_json::from_slice(&std::fs::read(entry?.path())?) {
                Ok(receipt) => receipt,
                Err(_) => continue,
            };
            if let Some(term) = term_for(&receipt, operation) {
                found = Some(term);
            }
        }
        let _ = ran;
        found.ok_or_else(|| format!("no receipt holds a term for {operation}").into())
    }
}

fn term_for(value: &Value, operation: &str) -> Option<Value> {
    match value {
        Value::Object(object) => {
            if object.contains_key("term_id")
                && object
                    .get("bounds")
                    .and_then(|bounds| bounds.get("operations"))
                    .is_some_and(|operations| operations == &json!([operation]))
            {
                return Some(value.clone());
            }
            object.values().find_map(|child| term_for(child, operation))
        }
        Value::Array(items) => items.iter().find_map(|child| term_for(child, operation)),
        _ => None,
    }
}

#[test]
fn a_run_authority_naming_packages_parses() -> TestResult {
    let project = Project::new("runx-store-packages-parse")?;
    project.skill("own", COUNTER)?;
    let ran = json(
        &project.run(
            "skills/own#bump",
            &[("key", "k"), ("attempt", "parsed")],
            Some(&packages(&["project:own"])),
        )?,
        0,
    )?;
    assert_eq!(store_result(&ran)["status"], "committed");
    let refused = project.run(
        "skills/own#bump",
        &[("key", "k"), ("attempt", "malformed")],
        Some(&packages(&["not a package"])),
    )?;
    assert!(!refused.status.success(), "{}", printed(&refused));
    Ok(())
}

// `private`: its own file in the runx home, every call needs a term, and
// only the owning package writes.

const NOTES: &str = r#"minimum_runx: 0.12.0
methods:
  keep:
    inputs:
      key: {type: string}
      text: {type: string}
      attempt: {type: string}
    steps:
      - id: before
        operation: private/key.inspect
        inputs: {stream: "said:personal", key: $input.key}
      - id: append
        operation: private/events.append
        idempotency_key: $input.attempt
        inputs:
          stream: "said:personal"
          key: $input.key
          expected_version: $steps.before.provider_operation.result.version
          type: notes.kept
          data: {text: $input.text}
  recall:
    inputs:
      key: {type: string}
    steps:
      - id: events
        operation: private/events.read
        inputs: {stream: "said:personal", key: $input.key}
  observe:
    inputs:
      key: {type: string}
      text: {type: string}
      attempt: {type: string}
    steps:
      - id: before
        operation: private/key.inspect
        inputs: {stream: "observed:habits", key: $input.key}
      - id: append
        operation: private/events.append
        idempotency_key: $input.attempt
        inputs:
          stream: "observed:habits"
          key: $input.key
          expected_version: $steps.before.provider_operation.result.version
          type: notes.observed
          data: {text: $input.text}
  search:
    inputs:
      query: {type: string}
    steps:
      - id: search
        operation: private/keys.search
        inputs: {stream: "said:personal", query: $input.query}
  count:
    steps:
      - id: before
        operation: store/key.inspect
        inputs: {stream: counts, key: k}
"#;

/// A term on the private instance for every operation on the notes stream.
fn private_term() -> Value {
    delegated(
        "private",
        &["read", "write"],
        &["private/*"],
        &["store.read", "store.append"],
        &["project:notes/said:personal"],
    )
}

#[test]
fn every_private_call_needs_a_term_and_lands_in_the_runx_home() -> TestResult {
    let project = Project::new("runx-store-private")?;
    project.skill("notes", NOTES)?;
    let keep = |key: &str, text: &str, attempt: &str, authority: Option<&Value>| {
        project.run(
            "skills/notes#keep",
            &[("key", key), ("text", text), ("attempt", attempt)],
            authority,
        )
    };

    // With no term, its owning package's own call is refused.
    let refused = keep("tea", "I take my tea black", "no-term", None)?;
    assert!(!refused.status.success(), "{}", printed(&refused));

    let term = private_term();
    let kept = json(
        &keep("tea", "I take my tea black", "first", Some(&term))?,
        0,
    )?;
    let appended = store_result(&kept);
    assert_eq!(
        appended["stream"], "project:notes/said:personal",
        "{appended}"
    );
    assert_eq!(appended["status"], "committed");
    assert!(project.root.join("home/private.sqlite").is_file());
    assert!(!project.root.join(".runx/private.sqlite").exists());

    // A term on private leaves store uncapped.
    let counted = project.result("skills/notes#count", &[], Some(&term))?;
    assert_eq!(counted["version"], 0, "{counted}");
    Ok(())
}

#[test]
fn private_refuses_a_home_that_fell_back_to_the_workspace() -> TestResult {
    let project = Project::new("runx-store-private-fallback")?;
    project.skill("notes", NOTES)?;
    let term = private_term();
    let mut command = crate::support::unsigned_runx_command_with_inherited_cwd();
    let ran = command
        .current_dir(&project.root)
        .env("RUNX_CWD", &project.root)
        .args([
            "skill",
            "skills/notes#keep",
            "-i",
            "key=tea",
            "-i",
            "text=black",
            "-i",
            "attempt=fell",
        ])
        .args(["--run-authority", &term.to_string(), "--json"])
        .output()?;
    assert!(!ran.status.success(), "{}", printed(&ran));
    assert!(printed(&ran).contains("HOME"), "{}", printed(&ran));
    Ok(())
}

#[test]
fn private_searches_current_entries_and_reads_a_query_as_plain_words() -> TestResult {
    let project = Project::new("runx-store-private-search")?;
    project.skill("notes", NOTES)?;
    let term = private_term();
    for (key, text, attempt) in [
        ("tea", "I take my tea black", "tea-1"),
        ("coffee", "Coffee only before noon", "coffee-1"),
        // A correction: the key's entry is its newest event.
        ("tea", "I take my tea with milk now", "tea-2"),
    ] {
        json(
            &project.run(
                "skills/notes#keep",
                &[("key", key), ("text", text), ("attempt", attempt)],
                Some(&term),
            )?,
            0,
        )?;
    }
    let search =
        |query: &str| project.result("skills/notes#search", &[("query", query)], Some(&term));

    let partial = search("black tea please")?;
    assert_eq!(partial["operation"], "keys.search");
    let keys = |found: &Value| -> Vec<Value> {
        found["matches"]
            .as_array()
            .map(|matches| matches.iter().map(|hit| hit["key"].clone()).collect())
            .unwrap_or_default()
    };
    // It shares some of the words, and only what's current: "black" was
    // corrected away, "tea" still matches.
    assert_eq!(keys(&partial), vec![json!("tea")], "{partial}");
    assert_eq!(
        partial["matches"][0]["last_event"]["data"]["text"],
        "I take my tea with milk now"
    );
    assert!(keys(&search("black")?).is_empty());
    // FTS5 syntax is plain words, never an error or a fan-out: OR and NEAR
    // are words here, and an unbalanced quote or parenthesis is no error.
    let mut either = keys(&search("coffee OR NEAR(tea)*")?);
    either.sort_by_key(ToString::to_string);
    assert_eq!(either, vec![json!("coffee"), json!("tea")]);
    assert_eq!(keys(&search("NEAR(tea")?), vec![json!("tea")]);
    assert_eq!(keys(&search("\"coffee noon")?), vec![json!("coffee")]);
    Ok(())
}

#[test]
fn a_private_instance_that_cannot_search_is_refused_when_it_is_configured() -> TestResult {
    let project = Project::new("runx-store-private-backend")?;
    for url in ["postgres://runx@localhost/runx", "redis://localhost:6379"] {
        let refused = project
            .runx()?
            .args([
                "connector",
                "use",
                "private",
                "--parameter",
                &format!("url={url}"),
                "--json",
            ])
            .output()?;
        assert!(!refused.status.success(), "{url}: {}", printed(&refused));
        assert!(
            printed(&refused).contains("keys.search"),
            "{}",
            printed(&refused)
        );
    }
    // store keeps every backend it has.
    json(
        &project
            .runx()?
            .args([
                "connector",
                "use",
                "store",
                "--parameter",
                "url=sqlite:elsewhere.sqlite",
                "--json",
            ])
            .output()?,
        0,
    )?;
    Ok(())
}

// A delegation holds one or more terms, each narrowing a term of the link
// before it, and a call is admitted when one of its terms covers it
// (authority.md).

/// The memory example as the notes package holds it: one delegation on
/// `private`, reading the person's words and reading and writing
/// observations, for notes alone. A plain narrowing: no actor, mandate or
/// principal.
fn memory_authority() -> Value {
    let term = |verbs: &[&str], scopes: &[&str], targets: &[&str]| {
        json!({
            "verbs": verbs,
            "bounds": {
                "operations": ["private/*"],
                "scopes": scopes,
                "targets": targets,
                "packages": ["project:notes"],
            },
        })
    };
    json!({
        "delegations": [{
            "connector": "private",
            "terms": [
                term(&["read"], &["store.read"], &["project:notes/said:*"]),
                term(
                    &["read", "write"],
                    &["store.read", "store.append"],
                    &["project:notes/observed:*"],
                ),
            ],
        }],
    })
}

#[test]
fn a_delegation_with_two_terms_admits_each_and_refuses_what_neither_covers() -> TestResult {
    let project = Project::new("runx-authority-two-terms")?;
    project.skill("notes", NOTES)?;
    let memory = memory_authority();
    let observe = |key: &str, attempt: &str, authority: &Value| {
        project.run(
            "skills/notes#observe",
            &[
                ("key", key),
                ("text", "takes tea black"),
                ("attempt", attempt),
            ],
            Some(authority),
        )
    };

    // Reading and writing observations: the second term.
    let observed = json(&observe("tea", "seen", &memory)?, 0)?;
    let appended = store_result(&observed);
    assert_eq!(appended["status"], "committed", "{appended}");
    assert_eq!(appended["stream"], "project:notes/observed:habits");
    // Reading the person's words: the first.
    let recalled = project.result("skills/notes#recall", &[("key", "tea")], Some(&memory))?;
    assert_eq!(
        recalled["stream"], "project:notes/said:personal",
        "{recalled}"
    );
    // Writing them: neither, so it's refused and nothing is kept.
    let kept = project.run(
        "skills/notes#keep",
        &[
            ("key", "tea"),
            ("text", "I take my tea black"),
            ("attempt", "said"),
        ],
        Some(&memory),
    )?;
    assert!(!kept.status.success(), "{}", printed(&kept));
    let recalled = project.result("skills/notes#recall", &[("key", "tea")], Some(&memory))?;
    assert_eq!(recalled["events"], json!([]), "{recalled}");

    // Under a run's packages naming notes, each term narrows that link.
    let mut narrowed = memory_authority();
    narrowed["packages"] = json!(["project:notes"]);
    let observed = json(&observe("coffee", "seen-under-packages", &narrowed)?, 0)?;
    assert_eq!(store_result(&observed)["status"], "committed");
    // A term that would widen it refuses, before any call.
    let mut widening = narrowed.clone();
    widening["delegations"][0]["terms"][1]["bounds"]["packages"] = json!(["*/*", "project:*"]);
    let refused = observe("juice", "widened", &widening)?;
    assert!(!refused.status.success(), "{}", printed(&refused));
    assert!(
        printed(&refused).contains("widens"),
        "{}",
        printed(&refused)
    );
    Ok(())
}

#[test]
fn one_delegation_per_instance_per_run() -> TestResult {
    let project = Project::new("runx-authority-one-delegation")?;
    project.skill("notes", NOTES)?;
    let mut twice = memory_authority();
    let again = twice["delegations"][0].clone();
    twice["delegations"]
        .as_array_mut()
        .ok_or("delegations is a list")?
        .push(again);
    let refused = project.run("skills/notes#recall", &[("key", "tea")], Some(&twice))?;
    assert!(!refused.status.success(), "{}", printed(&refused));
    assert!(
        printed(&refused).contains("one delegation"),
        "{}",
        printed(&refused)
    );
    Ok(())
}

/// A host delegating to an actor names it as the delegation's principal and
/// as the run's actor (§4.8). `private`'s instance is public, so whoever acts
/// is its principal: the mandate a delegation to another account's actor
/// takes is proven on a hosted instance, in the kernel's own tests.
#[test]
fn delegating_to_an_actor_names_it_as_the_runs_actor() -> TestResult {
    let project = Project::new("runx-authority-actor")?;
    project.skill("notes", NOTES)?;
    let actor = json!({"type": "principal", "uri": "runx:principal:assistant:test"});
    let mandate = json!({"type": "mandate", "uri": "runx:mandate:test"});
    let mut delegated = memory_authority();
    delegated["delegations"][0]["principal_ref"] = actor.clone();
    let with = |extra: Value| {
        let mut authority = delegated.clone();
        if let (Some(authority), Some(extra)) = (authority.as_object_mut(), extra.as_object()) {
            authority.extend(extra.clone());
        }
        authority
    };
    let observe = |attempt: &str, authority: &Value| {
        project.run(
            "skills/notes#observe",
            &[
                ("key", "tea"),
                ("text", "takes tea black"),
                ("attempt", attempt),
            ],
            Some(authority),
        )
    };
    for (attempt, missing) in [
        ("neither", json!({})),
        ("no-actor", json!({"mandate_ref": mandate})),
    ] {
        let refused = observe(attempt, &with(missing))?;
        assert!(
            !refused.status.success(),
            "{attempt}: {}",
            printed(&refused)
        );
    }
    let admitted = json(
        &observe(
            "both",
            &with(json!({"actor_ref": actor, "mandate_ref": mandate})),
        )?,
        0,
    )?;
    assert_eq!(store_result(&admitted)["status"], "committed");
    let term = project.receipt_with_term(&admitted, "private/events.append")?;
    assert_eq!(term["principal_ref"], actor, "{term}");
    Ok(())
}

// The chain sits beside the intent, never in it, so the same act through two
// wrappers is one act, with one provider key (D31).

/// A package that appends one fixed event at version 0 under the caller's
/// attempt: the same attempt is the same act.
const ONCE: &str = r#"minimum_runx: 0.12.0
methods:
  bump:
    inputs:
      key: {type: string}
      attempt: {type: string}
    steps:
      - id: append
        operation: store/events.append
        idempotency_key: $input.attempt
        inputs:
          stream: counts
          key: $input.key
          expected_version: 0
          type: once.done
          data:
            attempt: $input.attempt
  read:
    inputs:
      key: {type: string}
    steps:
      - id: events
        operation: store/events.read
        inputs:
          stream: counts
          key: $input.key
"#;

#[test]
fn the_same_act_through_two_wrappers_gets_one_provider_key() -> TestResult {
    let project = Project::with_registry("runx-store-one-key")?;
    project.publish("acme", "once", ONCE, &[])?;
    let dependency = "registry:acme/once@1.0.0";
    project.skill("wrap-a", &wrapper(dependency))?;
    project.skill("wrap-b", &wrapper(dependency))?;
    let bump = |wrapper: &str| -> TestResult<Value> {
        let ran = json(
            &project.run(
                &format!("skills/{wrapper}#bump"),
                &[("key", "k"), ("attempt", "the-act")],
                None,
            )?,
            0,
        )?;
        find_store_result(&ran, "events.append").ok_or_else(|| format!("{ran}").into())
    };
    let first = bump("wrap-a")?;
    assert_eq!(first["status"], "committed", "{first}");
    // Through the other wrapper the provider sees the first act's key, so it
    // answers with that event rather than a conflict at version 0.
    let second = bump("wrap-b")?;
    assert_eq!(second["status"], "replayed", "{second}");
    assert_eq!(second["event"], first["event"], "{second}");

    let ran = json(
        &project.run("skills/wrap-a#read", &[("key", "k")], None)?,
        0,
    )?;
    let read = find_store_result(&ran, "events.read").ok_or("nothing read")?;
    let events = read["events"].as_array().ok_or("events is a list")?;
    assert_eq!(events.len(), 1, "{read}");
    assert!(
        events[0]["idempotency_key"]
            .as_str()
            .is_some_and(|key| key.starts_with("runx:sha256:")),
        "{read}"
    );
    Ok(())
}

// Nobody can publish under `project`.

#[test]
fn nobody_publishes_under_project() -> TestResult {
    let project = Project::with_registry("runx-store-project-namespace")?;
    let skill = project.skill("github", COUNTER)?;
    let published = project
        .runx()?
        .arg("registry")
        .arg("publish")
        .arg(&skill)
        .args(["--publisher", "project", "--json"])
        .env("RUNX_API_TOKEN", crate::registry_fixture::TOKEN)
        .output()?;
    assert!(!published.status.success(), "{}", printed(&published));
    assert!(
        printed(&published).contains("project"),
        "{}",
        printed(&published)
    );
    assert!(
        project
            .registry()?
            .asked()
            .iter()
            .all(|asked| !asked.starts_with("POST")),
        "the publish reached the registry: {:?}",
        project.registry()?.asked()
    );

    // And no key, an operator's or runx's own, makes a signature under it
    // verify: a package signed as project/github never binds.
    let registry = project.registry()?;
    registry.publish(Publication::new(
        "project",
        "1.0.0",
        &manual("github"),
        Some(COUNTER),
    ))?;
    let mut install = project.runx()?;
    install.env(
        runx_runtime::registry::RUNX_REGISTRY_KEYS_ENV,
        registry.registry_keys("project")?,
    );
    let installed = install
        .args(["registry", "install", "project/github@1.0.0", "--json"])
        .output()?;
    assert!(!installed.status.success(), "{}", printed(&installed));
    Ok(())
}

/// Where a workspace's store lives.
fn workspace_store(root: &Path) -> PathBuf {
    root.join(".runx/store.sqlite")
}

// A store from before 0.12 is never read: it refuses to open, says 0.12
// doesn't read it, and says to move it aside.

#[test]
fn a_store_from_before_0_12_refuses_to_open_and_says_to_move_it_aside() -> TestResult {
    let project = Project::new("runx-store-before")?;
    project.skill("counter", COUNTER)?;
    let path = workspace_store(&project.root);
    std::fs::create_dir_all(path.parent().ok_or("a store has a directory")?)?;
    {
        let connection = rusqlite::Connection::open(&path)?;
        connection.execute_batch(
            "CREATE TABLE runx_events (resource TEXT NOT NULL, aggregate_id TEXT NOT NULL);
             INSERT INTO runx_events VALUES ('ops_projection', 'workspace');",
        )?;
        // runx 0.11's store: its application id, at version 2.
        connection.pragma_update(None, "application_id", 0x5255_4e53_i64)?;
        connection.pragma_update(None, "user_version", 2_i64)?;
    }
    let before = std::fs::read(&path)?;
    let refused = project.run("skills/counter#inspect", &[("key", "k")], None)?;
    assert!(!refused.status.success(), "{}", printed(&refused));
    for said in ["before runx 0.12", "doesn't read", "move the file aside"] {
        assert!(printed(&refused).contains(said), "{}", printed(&refused));
    }
    assert_eq!(
        std::fs::read(&path)?,
        before,
        "its rows are never read or moved"
    );

    // Moved aside, a new store starts.
    std::fs::rename(&path, path.with_extension("sqlite.0-11"))?;
    let inspected = project.result("skills/counter#inspect", &[("key", "k")], None)?;
    assert_eq!(inspected["version"], 0, "{inspected}");
    Ok(())
}
