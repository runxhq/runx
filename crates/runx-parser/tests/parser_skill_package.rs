#![allow(clippy::expect_used)]

use std::collections::{BTreeMap, BTreeSet};

use runx_parser::{SkillPackageSource, validate_skill_package};

fn manual(name: &str) -> String {
    format!("---\nname: {name}\ndescription: package test\n---\n\n# {name}\n\nOperate carefully.\n")
}

/// A manual carrying a `## Agent task: <name>` section for each agent task
/// its steps run.
fn manual_with_tasks(name: &str, tasks: &[&str]) -> String {
    let mut markdown = manual(name);
    for task in tasks {
        markdown.push_str(&format!("\n## Agent task: {task}\n\nDo the {task} task.\n"));
    }
    markdown
}

fn package(files: impl IntoIterator<Item = (&'static str, String)>) -> SkillPackageSource {
    SkillPackageSource {
        files: files
            .into_iter()
            .map(|(path, contents)| (path.to_owned(), contents.into_bytes()))
            .collect(),
        symlinks: BTreeSet::new(),
    }
}

#[test]
fn catalog_enforcement_keeps_explicit_internal_mock_methods_admissible() {
    let source = package([
        ("SKILL.md", manual("mock-pay")),
        (
            "run.yaml",
            "minimum_runx: 0.12.0\ncatalog:\n  visibility: internal\nmethods:\n  mock:\n    steps:\n      - id: mock\n        tool: {module: main.mjs}\n"
                .to_owned(),
        ),
        (
            "main.mjs",
            "export default function run() { return {}; }\n".to_owned(),
        ),
    ]);

    validate_skill_package(source).expect("explicit internal mock package must remain admissible");
}

#[test]
fn skill_package_preserves_exact_manual_and_validates_reachable_modules() {
    let manual = manual("demo");
    let source = package([
        ("SKILL.md", manual.clone()),
        (
            "run.yaml",
            "minimum_runx: 0.12.0\nmethods:\n  run:\n    steps:\n      - id: run\n        tool: {module: domain/main.mjs}\n"
                .to_owned(),
        ),
        (
            "domain/main.mjs",
            "import { project } from './project.js';\nexport default project;\n".to_owned(),
        ),
        (
            "domain/project.js",
            "export const project = (inputs) => inputs;\n".to_owned(),
        ),
    ]);

    let validated = validate_skill_package(source).expect("package must validate");

    assert_eq!(validated.manual_markdown, manual);
    assert!(validated.manual_digest.starts_with("sha256:"));
    assert!(validated.package_digest.starts_with("sha256:"));
    assert_eq!(
        validated
            .javascript_modules
            .keys()
            .cloned()
            .collect::<Vec<_>>(),
        vec!["domain/main.mjs", "domain/project.js"]
    );
}

#[test]
fn skill_package_manual_carries_only_agent_skills_fields() {
    let source = package([(
        "SKILL.md",
        r#"---
name: manual-inputs
description: inputs belong to run.yaml
inputs:
  message:
    type: string
    required: true
---

# Manual inputs
"#
        .to_owned(),
    )]);

    let error = validate_skill_package(source).expect_err("manual inputs must fail");
    assert!(error.to_string().contains("inputs"), "{error}");
}

#[test]
fn skill_package_allows_a_manual_only_context_artifact() {
    let source = package([(
        "SKILL.md",
        r#"---
name: operator-context
description: bounded advisory context
---

# Operator context

Use this evidence when reviewing a run.
"#
        .to_owned(),
    )]);

    let validated = validate_skill_package(source).expect("manual-only context should validate");
    assert!(validated.root_manifest().is_none());
}

#[test]
fn skill_package_tracks_declared_process_scripts_outside_module_bundle() {
    let source = package([
        ("SKILL.md", manual("process-script")),
        (
            "run.yaml",
            r#"minimum_runx: 0.12.0
methods:
  run:
    steps:
      - id: run
        tool: {command: node, args: [./run.mjs]}
"#
            .to_owned(),
        ),
        ("run.mjs", "console.log('{}');\n".to_owned()),
    ]);

    let validated = validate_skill_package(source).expect("process script package must validate");

    assert_eq!(
        validated.execution_files,
        BTreeSet::from(["run.mjs".to_owned()])
    );
    assert!(validated.javascript_modules.is_empty());
}

#[test]
fn skill_package_rejects_missing_declared_process_scripts() {
    let source = package([
        ("SKILL.md", manual("missing-process-script")),
        (
            "run.yaml",
            r#"minimum_runx: 0.12.0
methods:
  run:
    steps:
      - id: run
        tool: {command: node, args: [./missing.mjs]}
"#
            .to_owned(),
        ),
    ]);
    let error = validate_skill_package(source).expect_err("missing process script must fail");
    assert!(
        error
            .to_string()
            .contains("declared execution sidecar is missing")
    );
}

#[test]
fn skill_package_allows_explicit_selection_from_multiple_non_default_methods() {
    let source = package([
        (
            "SKILL.md",
            manual_with_tasks("explicit-selection", &["inspect", "apply"]),
        ),
        (
            "run.yaml",
            r#"minimum_runx: 0.12.0
methods:
  inspect:
    steps:
      - id: inspect
        agent: inspect
  apply:
    steps:
      - id: apply
        agent: apply
"#
            .to_owned(),
        ),
    ]);

    let validated = validate_skill_package(source).expect("package must admit explicit selection");

    assert_eq!(
        validated.root_manifest().map(|value| value.methods.len()),
        Some(2)
    );
}

#[test]
fn skill_package_rejects_cross_file_ambiguity_and_unsafe_sources() {
    let cases = [
        // The package's name is its manual's alone: run.yaml can't state
        // another.
        (
            package([
                ("SKILL.md", manual_with_tasks("demo", &["run"])),
                (
                    "run.yaml",
                    "minimum_runx: 0.12.0\nskill: other\nmethods:\n  run:\n    steps:\n      - id: run\n        agent: run\n".to_owned(),
                ),
            ]),
            "run.yaml.skill is not supported",
        ),
        (
            package([
                ("SKILL.md", manual("demo")),
                ("orphan.mjs", "export default {};\n".to_owned()),
            ]),
            "orphan.mjs",
        ),
        (
            package([
                ("SKILL.md", manual("demo")),
                (
                    "run.yaml",
                    "minimum_runx: 0.12.0\nmethods:\n  run:\n    steps:\n      - id: run\n        tool: {module: main.mjs}\n".to_owned(),
                ),
                (
                    "main.mjs",
                    "import value from 'left-pad';\nexport default value;\n".to_owned(),
                ),
            ]),
            "left-pad",
        ),
    ];

    for (source, expected) in cases {
        let error = validate_skill_package(source).expect_err("an ambiguous package must fail");
        assert!(error.to_string().contains(expected), "{expected}: {error}");
    }

    let mut links = BTreeSet::new();
    links.insert("main.mjs".to_owned());
    assert!(
        validate_skill_package(SkillPackageSource {
            files: BTreeMap::from([("SKILL.md".to_owned(), manual("demo").into_bytes())]),
            symlinks: links,
        })
        .is_err()
    );
}

#[test]
fn skill_package_validates_local_context_manuals() {
    let source = package([
        ("SKILL.md", manual_with_tasks("demo", &["review"])),
        (
            "run.yaml",
            r#"minimum_runx: 0.12.0
methods:
  run:
    result_from: [review]
    steps:
      - id: review
        agent: review
        context:
          skills:
            - ./context/rubric
"#
            .to_owned(),
        ),
        ("context/rubric/SKILL.md", manual("rubric")),
    ]);

    let validated = validate_skill_package(source).expect("context package must validate");
    assert_eq!(validated.context_skills, vec!["./context/rubric"]);
}

#[test]
fn skill_package_refuses_a_run_yaml_without_its_manual() {
    // A package's name is its manual's, so a run.yaml deeper in the package
    // is its own package's, beside the SKILL.md that names it.
    let source = package([
        ("SKILL.md", manual_with_tasks("operator", &["inspect"])),
        (
            "run.yaml",
            "minimum_runx: 0.12.0\nmethods:\n  inspect:\n    steps:\n      - id: inspect\n        agent: inspect\n".to_owned(),
        ),
        (
            "workflow/plan/run.yaml",
            "minimum_runx: 0.12.0\nmethods:\n  plan:\n    steps:\n      - id: plan\n        tool: {module: plan.mjs}\n".to_owned(),
        ),
        (
            "workflow/plan/plan.mjs",
            "export default (inputs) => inputs;\n".to_owned(),
        ),
    ]);

    let error = validate_skill_package(source.clone())
        .expect_err("a run.yaml without its manual must fail");
    assert!(
        error.to_string().contains(
            "workflow/plan/run.yaml: a run.yaml sits beside the SKILL.md that names its package"
        ),
        "{error}"
    );

    let mut owned = source;
    owned.files.insert(
        "workflow/plan/SKILL.md".to_owned(),
        manual("operator-plan").into_bytes(),
    );
    let validated = validate_skill_package(owned).expect("a nested package must validate");
    assert_eq!(
        validated.profiles.keys().cloned().collect::<Vec<_>>(),
        vec!["run.yaml"]
    );
    for path in [
        "workflow/plan/SKILL.md",
        "workflow/plan/run.yaml",
        "workflow/plan/plan.mjs",
    ] {
        assert!(validated.consumed_files.contains(path), "{path}");
    }
}

#[test]
fn skill_package_owns_operator_reference_markdown() {
    let source = package([
        ("SKILL.md", manual("operator")),
        ("references/guide.md", "# Guide\n".to_owned()),
        ("references/evidence.json", "{}\n".to_owned()),
        ("notes.md", "# Notes\n".to_owned()),
    ]);

    let validated = validate_skill_package(source).expect("reference package must validate");

    assert!(validated.consumed_files.contains("references/guide.md"));
    assert!(
        !validated
            .consumed_files
            .contains("references/evidence.json")
    );
    assert!(!validated.consumed_files.contains("notes.md"));
}

#[test]
fn eval_queries_is_a_list_of_query_and_should_trigger() {
    let with_queries = |queries: &str| {
        validate_skill_package(package([
            ("SKILL.md", manual("triggered")),
            ("eval_queries.json", queries.to_owned()),
        ]))
    };
    let validated = with_queries(
        r#"[{"query": "Fetch this page.", "should_trigger": true}, {"query": "Search the web.", "should_trigger": false}]"#,
    )
    .expect("well-formed trigger queries validate");
    assert!(validated.consumed_files.contains("eval_queries.json"));
    assert_eq!(
        serde_json::to_value(&validated.eval_queries).expect("queries serialize"),
        serde_json::json!([
            {"query": "Fetch this page.", "should_trigger": true},
            {"query": "Search the web.", "should_trigger": false}
        ])
    );

    for malformed in [
        "{}",
        "[]",
        r#"["Fetch this page."]"#,
        r#"[{"query": "", "should_trigger": true}]"#,
        r#"[{"query": "  ", "should_trigger": true}]"#,
        r#"[{"query": "Fetch this page.", "should_trigger": "yes"}]"#,
        r#"[{"query": "Fetch this page."}]"#,
        r#"[{"query": "Fetch this page.", "should_trigger": true, "skill": "web-fetch"}]"#,
    ] {
        let error = with_queries(malformed).expect_err(malformed);
        assert_eq!(
            error.to_string(),
            "eval_queries.json: is a list of { query, should_trigger }",
            "{malformed}"
        );
    }
}

#[test]
fn evals_json_states_the_packages_evals_under_its_name() {
    let with_evals = |evals: &str| {
        validate_skill_package(package([
            ("SKILL.md", manual("triggered")),
            ("evals/evals.json", evals.to_owned()),
        ]))
    };
    let validated = with_evals(
        r#"{"skill_name": "triggered", "evals": [
            {"id": 1, "prompt": "Fetch this page.", "expected_output": "The page's text, by digest.", "files": []},
            {"id": 2, "prompt": "Fetch it twice.", "expected_output": "One read.", "files": [], "expectations": ["It reads once."], "method": "fetch"}
        ]}"#,
    )
    .expect("well-formed evals validate");
    assert!(validated.consumed_files.contains("evals/evals.json"));
    assert_eq!(
        serde_json::to_value(&validated.evals).expect("evals serialize"),
        serde_json::json!([
            {"id": 1, "prompt": "Fetch this page.", "expected_output": "The page's text, by digest."},
            {"id": 2, "prompt": "Fetch it twice.", "expected_output": "One read.", "expectations": ["It reads once."], "method": "fetch"}
        ])
    );

    for (malformed, refusal) in [
        ("[]", "evals/evals.json: is the Agent Skills evals format"),
        (
            r#"{"skill_name": "other", "evals": []}"#,
            "evals/evals.json: skill_name is the package's name, triggered",
        ),
        (
            r#"{"skill_name": "triggered", "evals": [{"id": 1, "expected_output": "x"}]}"#,
            "evals/evals.json: is the Agent Skills evals format",
        ),
        (
            r#"{"skill_name": "triggered", "evals": [{"id": 1, "prompt": "p", "expected_output": "x", "grader": "me"}]}"#,
            "evals/evals.json: is the Agent Skills evals format",
        ),
        (
            r#"{"skill_name": "triggered", "evals": [{"id": 1, "prompt": "p", "expected_output": "x"}, {"id": 1, "prompt": "q", "expected_output": "y"}]}"#,
            "evals/evals.json: eval 1 is listed twice",
        ),
    ] {
        let error = with_evals(malformed).expect_err(malformed).to_string();
        assert!(error.starts_with(refusal), "{malformed}: {error}");
    }
}

/// A file an eval names is the package's own: it must be there, as text,
/// and it is part of what the package distributes.
#[test]
fn an_evals_files_are_the_packages_own() {
    let evals = r#"{"skill_name": "triggered", "evals": [
        {"id": 1, "prompt": "Build on the prior run.", "expected_output": "It reuses it.", "files": ["evals/files/prior.json"]}
    ]}"#;
    let validated = validate_skill_package(package([
        ("SKILL.md", manual("triggered")),
        ("evals/evals.json", evals.to_owned()),
        ("evals/files/prior.json", r#"{"query": "proof"}"#.to_owned()),
    ]))
    .expect("an eval with its file validates");
    assert!(validated.consumed_files.contains("evals/files/prior.json"));

    let error = validate_skill_package(package([
        ("SKILL.md", manual("triggered")),
        ("evals/evals.json", evals.to_owned()),
    ]))
    .expect_err("an eval naming a file the package lacks is refused")
    .to_string();
    assert_eq!(
        error,
        "evals/evals.json: eval 1 names evals/files/prior.json, which the package doesn't hold"
    );
}

#[test]
fn skill_package_qualifies_nested_package_errors() {
    let source = package([
        ("SKILL.md", manual("operator")),
        (
            "context/rubric/SKILL.md",
            "---\nname: rubric\ndescription: rubric fixture.\nallowed_tools: [example.echo]\n---\n"
                .to_owned(),
        ),
    ]);

    let error = validate_skill_package(source).expect_err("invalid nested manual must fail");

    assert!(
        error.to_string().starts_with("context/rubric/SKILL.md"),
        "nested error lost its package path: {error}"
    );
}

#[test]
fn skill_package_parses_owned_test_fixtures() {
    let source = package([
        ("SKILL.md", manual_with_tasks("operator", &["inspect"])),
        (
            "run.yaml",
            "minimum_runx: 0.12.0\nmethods:\n  inspect:\n    steps:\n      - id: inspect\n        agent: inspect\n".to_owned(),
        ),
        (
            "tests/inspect.yaml",
            r#"name: inspect
method: inspect
inputs:
  objective: inspect safely
expect:
  status: needs_agent
"#
            .to_owned(),
        ),
    ]);

    let validated = validate_skill_package(source).expect("fixture package must validate");
    let fixture = validated
        .test_fixtures
        .get("tests/inspect.yaml")
        .expect("owned fixture must be in the aggregate");

    assert_eq!(fixture.name, "inspect");
    assert_eq!(fixture.method.as_deref(), Some("inspect"));

    // A test runs a method its package has, as a test case's method did.
    let mut unknown = validated.source;
    unknown.files.insert(
        "tests/unknown.yaml".to_owned(),
        b"name: unknown\nmethod: missing\nexpect:\n  status: sealed\n".to_vec(),
    );
    let error = validate_skill_package(unknown).expect_err("a test of an unknown method must fail");
    assert!(
        error
            .to_string()
            .contains("tests/unknown.yaml: method missing is not one of the package's methods"),
        "{error}"
    );
}

#[test]
fn skill_package_consumes_conventional_workflow_test_closure() {
    // A workflow a test ran is a fixture package under fixtures/, as the
    // converter writes it, with the script beside the workflow staged with the
    // rest of fixtures/.
    let source = package([
        ("SKILL.md", manual_with_tasks("operator", &["inspect"])),
        (
            "run.yaml",
            "minimum_runx: 0.12.0\nmethods:\n  inspect:\n    steps:\n      - id: inspect\n        agent: inspect\n".to_owned(),
        ),
        (
            "tests/inspect.yaml",
            r#"name: inspect
package: fixtures/inspect
expect:
  status: sealed
"#
            .to_owned(),
        ),
        (
            "fixtures/inspect/SKILL.md",
            "---\nname: inspect\ndescription: A test fixture that composes operator with the packages it calls.\n---\n\nUsed only by operator's tests.\n".to_owned(),
        ),
        (
            "fixtures/inspect/run.yaml",
            r#"minimum_runx: 0.12.0
catalog:
  visibility: internal
methods:
  run:
    steps:
      - id: setup
        tool: {command: node, args: [../support/setup.mjs]}
"#
            .to_owned(),
        ),
        (
            "fixtures/support/setup.mjs",
            "process.stdout.write('{}\\n');\n".to_owned(),
        ),
    ]);

    let validated = validate_skill_package(source).expect("test closure must validate");

    for path in [
        "tests/inspect.yaml",
        "fixtures/inspect/SKILL.md",
        "fixtures/inspect/run.yaml",
        "fixtures/support/setup.mjs",
    ] {
        assert!(
            validated.consumed_files.contains(path),
            "{path} is missing from parser-owned package material",
        );
    }

    let mut unpackaged = validated.source;
    unpackaged.files.remove("fixtures/inspect/SKILL.md");
    unpackaged.files.remove("fixtures/inspect/run.yaml");
    let error = validate_skill_package(unpackaged)
        .expect_err("a test's package must be a package of its own");
    assert!(
        error
            .to_string()
            .contains("tests/inspect.yaml: package \"fixtures/inspect\" is not a fixture package"),
        "{error}"
    );
}

#[test]
fn skill_package_stages_its_fixtures_whole_for_tests() {
    let source = package([
        ("SKILL.md", manual_with_tasks("operator", &["inspect"])),
        (
            "run.yaml",
            r#"minimum_runx: 0.12.0
methods:
  inspect:
    steps:
      - id: inspect
        agent: inspect
"#
            .to_owned(),
        ),
        (
            "tests/inspect.yaml",
            r#"name: inspect
method: inspect
inputs: {}
expect:
  status: needs_agent
"#
            .to_owned(),
        ),
        (
            "fixtures/root-helper.mjs",
            "export default 'root';\n".to_owned(),
        ),
    ]);

    let validated = validate_skill_package(source).expect("test files must validate");

    assert_eq!(
        validated.test_files,
        BTreeSet::from([
            "fixtures/root-helper.mjs".to_owned(),
            "tests/inspect.yaml".to_owned(),
        ])
    );
}

#[test]
fn skill_package_validates_bundled_tools_and_records_their_source_closure() {
    let source = package([
        ("SKILL.md", manual("bundled-tool")),
        (
            "run.yaml",
            r#"minimum_runx: 0.12.0
methods:
  run:
    result_from: [invoke]
    steps:
      - id: invoke
        tool: example.echo
        inputs: {}
"#
            .to_owned(),
        ),
        (
            "tools/example/echo/manifest.json",
            r#"{
  "schema": "runx.tool.manifest.v1",
  "name": "example.echo",
  "process": {
    "command": "node",
    "args": ["./run.mjs"]
  }
}
"#
            .to_owned(),
        ),
        (
            "tools/example/echo/run.mjs",
            "import { echo } from './echo.mjs';\nprocess.stdout.write(echo());\n".to_owned(),
        ),
        (
            "tools/example/echo/echo.mjs",
            "export const echo = () => '{}';\n".to_owned(),
        ),
    ]);

    let validated = validate_skill_package(source).expect("bundled tool package must validate");
    let manifest_path = "tools/example/echo/manifest.json";
    let tool = validated
        .tools
        .get(manifest_path)
        .expect("bundled tool must be typed package truth");

    assert_eq!(tool.tool.name, "example.echo");
    assert_eq!(
        tool.source_files,
        BTreeSet::from([
            "tools/example/echo/echo.mjs".to_owned(),
            "tools/example/echo/run.mjs".to_owned(),
        ])
    );
    assert!(validated.execution_files.contains(manifest_path));
    assert!(
        validated
            .execution_files
            .contains("tools/example/echo/echo.mjs")
    );
    assert!(
        validated
            .consumed_files
            .contains("tools/example/echo/echo.mjs")
    );
}

#[test]
fn skill_package_rejects_invalid_or_misplaced_bundled_tool_manifests() {
    for (manifest_path, manifest, expected) in [
        (
            "tools/example/echo/manifest.json",
            "{}",
            "schema is required",
        ),
        (
            "tools/example/echo/manifest.json",
            r#"{
  "schema": "runx.tool.manifest.v1",
  "name": "example.other",
  "process": {
    "command": "node",
    "args": ["./run.mjs"]
  }
}"#,
            "must match its catalog path",
        ),
    ] {
        let source = package([
            ("SKILL.md", manual("invalid-bundled-tool")),
            (manifest_path, manifest.to_owned()),
            (
                "tools/example/echo/run.mjs",
                "process.stdout.write('{}');\n".to_owned(),
            ),
        ]);

        let error = validate_skill_package(source).expect_err("invalid tool must fail package");
        assert!(
            error.to_string().contains(expected),
            "unexpected error for {manifest_path}: {error}"
        );
    }
}

#[test]
fn skill_package_rejects_unbound_bundled_tool_dependencies() {
    let source = package([
        ("SKILL.md", manual("missing-tool-source")),
        (
            "tools/example/echo/manifest.json",
            r#"{
  "schema": "runx.tool.manifest.v1",
  "name": "example.echo",
  "process": {
    "command": "node",
    "args": ["./run.mjs"]
  }
}"#
            .to_owned(),
        ),
        (
            "tools/example/echo/run.mjs",
            "import './missing.mjs';\n".to_owned(),
        ),
    ]);

    let error = validate_skill_package(source).expect_err("missing dependency must fail package");
    assert!(error.to_string().contains("resolves to missing file"));
}

#[test]
fn skill_package_rejects_a_node_tool_that_imports_uncompiled_typescript() {
    let source = package([
        ("SKILL.md", manual("typescript-tool-source")),
        (
            "tools/example/echo/manifest.json",
            r#"{
  "schema": "runx.tool.manifest.v1",
  "name": "example.echo",
  "process": {
    "command": "node",
    "args": ["./run.mjs"]
  }
}"#
            .to_owned(),
        ),
        (
            "tools/example/echo/run.mjs",
            "import './src/index.ts';\n".to_owned(),
        ),
        (
            "tools/example/echo/src/index.ts",
            "export const ok = true;\n".to_owned(),
        ),
    ]);

    let error = validate_skill_package(source).expect_err("a TypeScript import must fail package");
    assert!(
        error.to_string().contains("imports uncompiled TypeScript"),
        "{error}"
    );
}

#[test]
fn a_mark_beside_the_manual_ships_with_the_package_and_a_bad_one_fails_it() {
    let mark = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M4 4h16v16H4z" fill="#0f62fe"/></svg>"##;
    let validated = validate_skill_package(package([
        ("SKILL.md", manual("marked")),
        ("mark.svg", mark.to_owned()),
    ]))
    .expect("a package with a valid mark validates");
    assert!(validated.consumed_files.contains("mark.svg"));

    let nested = validate_skill_package(package([
        ("SKILL.md", manual("parent")),
        ("child/SKILL.md", manual("child")),
        ("child/mark.svg", mark.to_owned()),
    ]))
    .expect("a nested package's mark validates");
    assert!(nested.consumed_files.contains("child/mark.svg"));

    let hostile = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><script>alert(1)</script></svg>"#;
    let error = validate_skill_package(package([
        ("SKILL.md", manual("marked")),
        ("mark.svg", hostile.to_owned()),
    ]))
    .expect_err("a mark that can run script fails the package");
    assert!(error.to_string().contains("mark.svg"), "{error}");
}

#[test]
fn a_declared_category_must_be_a_skill_category_of_the_taxonomy() {
    let with_category = |category: &str| {
        package([
            ("SKILL.md", manual("categorized")),
            (
                "run.yaml",
                format!(
                    "minimum_runx: 0.12.0\ncatalog:\n  visibility: internal\n  category: {category}\nmethods:\n  main:\n    steps:\n      - id: run\n        tool: {{module: main.mjs}}\n"
                ),
            ),
            (
                "main.mjs",
                "export default function run() { return {}; }\n".to_owned(),
            ),
        ])
    };
    validate_skill_package(with_category("research")).expect("a taxonomy skill category validates");
    for refused in ["communication", "Research", "uncategorized", "payment"] {
        let error = validate_skill_package(with_category(refused)).expect_err(refused);
        assert!(error.to_string().contains("category"), "{refused}: {error}");
    }
}

#[test]
fn every_taxonomy_category_mark_passes_the_mark_check() {
    let marks =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../registry/marks/categories");
    let mut checked = 0;
    for entry in std::fs::read_dir(&marks).expect("category marks") {
        let path = entry.expect("entry").path();
        let bytes = std::fs::read(&path).expect("mark bytes");
        assert_eq!(
            runx_parser::mark::validate_mark_svg(&bytes),
            Ok(()),
            "{}",
            path.display()
        );
        checked += 1;
    }
    assert!(checked > 0);
}

/// A method is shown by its own description. Only a package's one method
/// borrows SKILL.md's, so a package with several never shows the same words
/// for two of them.
#[test]
fn a_method_is_described_in_its_own_words_and_only_a_lone_method_borrows_the_packages()
-> Result<(), String> {
    let manifest = |yaml: &str| {
        runx_parser::parse_method_manifest_yaml(yaml)
            .map_err(|error| error.to_string())
            .and_then(|raw| {
                runx_parser::validate_method_manifest("demo", raw)
                    .map_err(|error| error.to_string())
            })
    };
    let step = "    steps:\n      - id: run\n        tool: {module: run.mjs, export: run}\n";
    let lone = manifest(&format!("minimum_runx: 0.12.0\nmethods:\n  run:\n{step}"))?;
    let run = &lone.methods["run"];
    assert_eq!(
        lone.method_description(run, Some("The package.")),
        Some("The package.")
    );

    let several = manifest(&format!(
        "minimum_runx: 0.12.0\ndefault_method: read\nmethods:\n  read:\n    description: Read one record.\n{step}  write:\n{step}"
    ))?;
    assert_eq!(
        several.method_description(&several.methods["read"], Some("The package.")),
        Some("Read one record.")
    );
    assert_eq!(
        several.method_description(&several.methods["write"], Some("The package.")),
        None
    );
    Ok(())
}
