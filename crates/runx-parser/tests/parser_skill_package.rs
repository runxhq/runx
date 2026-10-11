#![allow(clippy::expect_used)]

use std::collections::{BTreeMap, BTreeSet};

use runx_parser::{SkillPackageSource, validate_skill_package};

fn manual(name: &str) -> String {
    format!("---\nname: {name}\ndescription: package test\n---\n\n# {name}\n\nOperate carefully.\n")
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
fn catalog_enforcement_keeps_explicit_internal_mock_runners_admissible() {
    let source = package([
        ("SKILL.md", manual("mock-pay")),
        (
            "run.yaml",
            "skill: mock-pay\ncatalog:\n  kind: skill\n  audience: system\n  visibility: internal\n  role: harness-fixture\n  part_of: [runx/spend]\n  execution: execute\n  completion: runtime_receipt\n  requires_adapter: false\n  approval: none\nrunners:\n  mock:\n    default: true\n    steps:\n      - id: mock\n        tool: { module: main.mjs }\n"
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
            "skill: demo\nrunners:\n  run:\n    default: true\n    steps:\n      - id: run\n        tool: { module: domain/main.mjs }\n"
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
            r#"skill: process-script
runners:
  run:
    default: true
    steps:
      - id: run
        tool: { command: node, args: [./run.mjs] }
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
            r#"skill: missing-process-script
runners:
  run:
    default: true
    steps:
      - id: run
        tool: { command: node, args: [./missing.mjs] }
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
fn skill_package_allows_explicit_selection_from_multiple_non_default_runners() {
    let source = package([
        ("SKILL.md", manual("explicit-selection")),
        (
            "run.yaml",
            r#"skill: explicit-selection
runners:
  inspect:
    steps:
      - id: inspect
        agent: { role: agent, task: inspect }
  apply:
    steps:
      - id: apply
        agent: { role: agent, task: apply }
"#
            .to_owned(),
        ),
    ]);

    let validated = validate_skill_package(source).expect("package must admit explicit selection");

    assert_eq!(
        validated.root_manifest().map(|value| value.runners.len()),
        Some(2)
    );
}

#[test]
fn skill_package_rejects_cross_file_ambiguity_and_unsafe_sources() {
    let cases = [
        package([
            ("SKILL.md", manual("demo")),
            (
                "run.yaml",
                "skill: other\nrunners:\n  run:\n    steps:\n      - id: run\n        agent: { role: agent, task: run }\n".to_owned(),
            ),
        ]),
        package([
            ("SKILL.md", manual("demo")),
            ("orphan.mjs", "export default {};\n".to_owned()),
        ]),
        package([
            ("SKILL.md", manual("demo")),
            (
                "run.yaml",
                "skill: demo\nrunners:\n  run:\n    steps:\n      - id: run\n        tool: { module: main.mjs }\n".to_owned(),
            ),
            (
                "main.mjs",
                "import value from 'left-pad';\nexport default value;\n".to_owned(),
            ),
        ]),
    ];

    for source in cases {
        assert!(validate_skill_package(source).is_err());
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
        ("SKILL.md", manual("demo")),
        (
            "run.yaml",
            r#"skill: demo
runners:
  run:
    result_from: [review]
    steps:
      - id: review
        agent: { role: reviewer, task: review }
        context_skills:
          - ./context/rubric
"#
            .to_owned(),
        ),
        ("context/rubric/SKILL.md", manual("rubric")),
    ]);

    let validated = validate_skill_package(source).expect("context package must validate");
    assert_eq!(validated.context_skill_refs, vec!["./context/rubric"]);
}

#[test]
fn skill_package_manual_owns_nested_execution_profiles() {
    let source = package([
        ("SKILL.md", manual("operator")),
        (
            "run.yaml",
            "skill: operator\nrunners:\n  inspect:\n    steps:\n      - id: inspect\n        agent: { role: agent, task: inspect }\n".to_owned(),
        ),
        (
            "graph/plan/run.yaml",
            "skill: operator-plan\nrunners:\n  plan:\n    steps:\n      - id: plan\n        tool: { module: plan.mjs }\n".to_owned(),
        ),
        (
            "graph/plan/plan.mjs",
            "export default (inputs) => inputs;\n".to_owned(),
        ),
    ]);

    let validated = validate_skill_package(source).expect("owned profiles must validate");

    assert_eq!(
        validated.profiles.keys().cloned().collect::<Vec<_>>(),
        vec!["graph/plan/run.yaml", "run.yaml"]
    );
    assert!(
        validated
            .javascript_modules
            .contains_key("graph/plan/plan.mjs")
    );
    assert_eq!(validated.manual_markdown, manual("operator"));
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
fn skill_package_parses_owned_harness_fixtures() {
    let source = package([
        ("SKILL.md", manual("operator")),
        (
            "run.yaml",
            "skill: operator\nrunners:\n  inspect:\n    steps:\n      - id: inspect\n        agent: { role: agent, task: inspect }\n".to_owned(),
        ),
        (
            "fixtures/inspect.yaml",
            r#"name: inspect
kind: skill
target: ..
runner: inspect
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
        .harness_fixtures
        .get("fixtures/inspect.yaml")
        .expect("owned fixture must be in the aggregate");

    assert_eq!(fixture.name, "inspect");
    assert_eq!(fixture.runner.as_deref(), Some("inspect"));
}

#[test]
fn skill_package_consumes_conventional_graph_harness_closure() {
    let source = package([
        ("SKILL.md", manual("operator")),
        (
            "run.yaml",
            "skill: operator\nrunners:\n  inspect:\n    steps:\n      - id: inspect\n        agent: { role: agent, task: inspect }\n".to_owned(),
        ),
        (
            "fixtures/inspect.yaml",
            r#"name: inspect
kind: graph
target: ../harness/inspect.graph.yaml
expect:
  status: sealed
"#
            .to_owned(),
        ),
        (
            "harness/inspect.graph.yaml",
            r#"name: inspect
steps:
  - id: setup
    tool: { command: node, args: [./setup.mjs] }
"#
            .to_owned(),
        ),
        (
            "harness/setup.mjs",
            "process.stdout.write('{}\\n');\n".to_owned(),
        ),
    ]);

    let validated = validate_skill_package(source).expect("harness closure must validate");

    for path in [
        "fixtures/inspect.yaml",
        "harness/inspect.graph.yaml",
        "harness/setup.mjs",
    ] {
        assert!(
            validated.consumed_files.contains(path),
            "{path} is missing from parser-owned package material",
        );
    }
}

#[test]
fn skill_package_admits_explicit_profile_relative_harness_files() {
    let source = package([
        ("SKILL.md", manual("operator")),
        (
            "run.yaml",
            r#"skill: operator
runners:
  inspect:
    steps:
      - id: inspect
        agent: { role: agent, task: inspect }
harness:
  files:
    - fixtures/root-helper.mjs
  cases:
    - name: inspect
      runner: inspect
      inputs: {}
      expect:
        status: needs_agent
"#
            .to_owned(),
        ),
        (
            "graph/plan/run.yaml",
            r#"skill: demo-plan
runners:
  plan:
    steps:
      - id: plan
        agent: { role: agent, task: plan }
harness:
  files:
    - fixtures/plan-helper.mjs
  cases:
    - name: plan
      runner: plan
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
        (
            "graph/plan/fixtures/plan-helper.mjs",
            "export default 'plan';\n".to_owned(),
        ),
    ]);

    let validated = validate_skill_package(source).expect("harness files must validate");

    assert_eq!(
        validated.harness_files,
        BTreeSet::from([
            "fixtures/root-helper.mjs".to_owned(),
            "graph/plan/fixtures/plan-helper.mjs".to_owned(),
        ])
    );
}

#[test]
fn skill_package_rejects_missing_or_unsafe_harness_files() {
    for declared in [
        "fixtures/missing.mjs",
        "../outside.mjs",
        "tools/not-a-fixture.mjs",
        "fixtures/../outside.mjs",
    ] {
        let source = package([
            ("SKILL.md", manual("operator")),
            (
                "run.yaml",
                format!(
                    "skill: operator\nrunners:\n  inspect:\n    steps:\n      - id: inspect\n        agent: {{ role: agent, task: inspect }}\nharness:\n  files:\n    - {declared}\n  cases:\n    - name: inspect\n      runner: inspect\n      inputs: {{}}\n      expect:\n        status: needs_agent\n"
                ),
            ),
        ]);

        let error = validate_skill_package(source).expect_err("unsafe harness file must fail");
        assert!(
            error.to_string().contains("harness file"),
            "unexpected rejection for {declared}: {error}"
        );
    }
}

#[test]
fn skill_package_validates_bundled_tools_and_records_their_source_closure() {
    let source = package([
        ("SKILL.md", manual("bundled-tool")),
        (
            "run.yaml",
            r#"skill: bundled-tool
runners:
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
  "source": {
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
        .tool_at(manifest_path)
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
  "source": { "command": "node", "args": ["./run.mjs"] }
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
  "source": { "command": "node", "args": ["./run.mjs"] }
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
                    "skill: categorized\ncatalog:\n  kind: skill\n  audience: builder\n  visibility: internal\n  role: canonical\n  category: {category}\nrunners:\n  main:\n    default: true\n    steps:\n      - id: run\n        tool: {{ module: main.mjs }}\n"
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
    assert!(checked >= runx_contracts::registry_categories().len());
}
