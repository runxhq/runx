use serde::Deserialize;

use runx_parser::{
    ExecutionGraph, SkillInstallOrigin, SkillRunnerManifest, ValidatedSkill, ValidatedSkillInstall,
    ValidatedTool, parse_graph_yaml, parse_runner_manifest_yaml, parse_skill_markdown,
    parse_tool_manifest_json, parse_tool_manifest_yaml, validate_graph, validate_runner_manifest,
    validate_skill, validate_skill_install, validate_tool_manifest,
};

const GRAPH_FIXTURES: &[&str] = &[
    include_str!("../../../fixtures/parser/graphs/parse-malformed-yaml.json"),
    include_str!("../../../fixtures/parser/graphs/sequential-context.json"),
    include_str!("../../../fixtures/parser/graphs/step-kinds.json"),
    include_str!("../../../fixtures/parser/graphs/tool-and-policy.json"),
    include_str!("../../../fixtures/parser/graphs/validation-missing-step-id.json"),
    include_str!("../../../fixtures/parser/graphs/validation-two-step-kinds.json"),
];

const SKILL_FIXTURES: &[&str] = &[
    include_str!("../../../fixtures/parser/skills/portable-manual.json"),
    include_str!("../../../fixtures/parser/skills/validation-manual-inputs.json"),
];

const RUNNER_MANIFEST_FIXTURES: &[&str] = &[
    include_str!("../../../fixtures/parser/runner-manifests/harness-basic.json"),
    include_str!(
        "../../../fixtures/parser/runner-manifests/validation-harness-unknown-runner.json"
    ),
    include_str!(
        "../../../fixtures/parser/runner-manifests/validation-invalid-reflect-policy.json"
    ),
];

const TOOL_MANIFEST_FIXTURES: &[&str] = &[
    include_str!("../../../fixtures/parser/tool-manifests/process.json"),
    include_str!("../../../fixtures/parser/tool-manifests/validation-source-missing-command.json"),
];

const INSTALL_FIXTURES: &[&str] = &[include_str!(
    "../../../fixtures/parser/installs/installed-skill.json"
)];

#[derive(Debug, Deserialize)]
struct GraphFixture {
    input: YamlInput,
    expected: GraphExpected,
}

#[derive(Debug, Deserialize)]
struct SkillFixture {
    input: MarkdownInput,
    expected: SkillExpected,
}

#[derive(Debug, Deserialize)]
struct RunnerManifestFixture {
    input: YamlInput,
    expected: RunnerManifestExpected,
}

#[derive(Debug, Deserialize)]
struct ToolManifestFixture {
    input: ToolInput,
    expected: ToolExpected,
}

#[derive(Debug, Deserialize)]
struct InstallFixture {
    input: InstallInput,
    expected: InstallExpected,
}

#[derive(Debug, Deserialize)]
struct YamlInput {
    yaml: String,
}

#[derive(Debug, Deserialize)]
struct MarkdownInput {
    markdown: String,
}

#[derive(Debug, Deserialize)]
struct ToolInput {
    yaml: Option<String>,
    json: Option<String>,
}

#[derive(Debug, Deserialize)]
struct InstallInput {
    markdown: String,
    origin: SkillInstallOrigin,
}

#[derive(Debug, Deserialize)]
struct GraphExpected {
    validated: Option<ExecutionGraph>,
    rejection: Option<Rejection>,
}

#[derive(Debug, Deserialize)]
struct SkillExpected {
    validated: Option<ValidatedSkill>,
    rejection: Option<Rejection>,
}

#[derive(Debug, Deserialize)]
struct RunnerManifestExpected {
    validated: Option<SkillRunnerManifest>,
    rejection: Option<Rejection>,
}

#[derive(Debug, Deserialize)]
struct ToolExpected {
    validated: Option<ValidatedTool>,
    rejection: Option<Rejection>,
}

#[derive(Debug, Deserialize)]
struct InstallExpected {
    validated: Option<ValidatedSkillInstall>,
}

#[derive(Debug, Deserialize)]
struct Rejection {
    kind: RejectionKind,
    message: String,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum RejectionKind {
    Parse,
    Validation,
}

#[test]
fn graph_fixtures_match_expected() -> Result<(), String> {
    for fixture_json in GRAPH_FIXTURES {
        let fixture: GraphFixture =
            serde_json::from_str(fixture_json).map_err(|error| error.to_string())?;
        assert_graph_fixture(fixture)?;
    }
    Ok(())
}

#[test]
fn skill_fixtures_match_expected() -> Result<(), String> {
    for fixture_json in SKILL_FIXTURES {
        let fixture: SkillFixture =
            serde_json::from_str(fixture_json).map_err(|error| error.to_string())?;
        assert_skill_fixture(fixture)?;
    }
    Ok(())
}

#[test]
fn catalog_carries_category_tags_owner_and_links() -> Result<(), String> {
    let manifest = validate_runner_manifest(
        parse_runner_manifest_yaml(
            r#"
skill: docs-demo
catalog:
  kind: skill
  audience: public
  visibility: public
  role: canonical
  category: content
  tags: [docs, writing]
  owner: zhtwangk
  links:
    source: https://github.com/example/docs-demo
runners:
  main:
    steps:
      - id: write
        agent: { role: writer, task: write }
"#,
        )
        .map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;

    let catalog = manifest.catalog.ok_or("catalog should be present")?;
    assert_eq!(catalog.category.as_deref(), Some("content"));
    assert_eq!(catalog.tags, vec!["docs".to_owned(), "writing".to_owned()]);
    assert_eq!(catalog.owner.as_deref(), Some("zhtwangk"));
    assert_eq!(
        catalog.links.get("source").map(String::as_str),
        Some("https://github.com/example/docs-demo")
    );
    Ok(())
}

#[test]
fn catalog_rejects_noncanonical_owner_and_non_https_links() -> Result<(), String> {
    for (field, value) in [
        ("owner", "ZHTWangK"),
        ("links", "{source: http://example.com/repo}"),
        ("tags", "[docs, docs]"),
    ] {
        let raw = parse_runner_manifest_yaml(&format!(
            "skill: docs-demo\ncatalog:\n  kind: skill\n  audience: public\n  visibility: public\n  role: canonical\n  {field}: {value}\nrunners:\n  main:\n    steps:\n      - id: write\n        agent: {{ role: writer, task: write }}\n"
        ))
        .map_err(|error| error.to_string())?;
        assert!(
            validate_runner_manifest(raw).is_err(),
            "catalog.{field}: {value} should be rejected"
        );
    }
    Ok(())
}

#[test]
fn strict_frontmatter_applies_the_agent_skills_rules() -> Result<(), String> {
    let accepted = "---\nname: docs-demo\ndescription: Demo skill.\nlicense: Apache-2.0\ncompatibility: Requires git.\nmetadata:\n  author: runx\nallowed-tools: Bash(git:*) Read\n---\n# Demo\n";
    validate_skill(parse_skill_markdown(accepted).map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())?;

    let long_description = "x".repeat(1025);
    let long_compatibility = "x".repeat(501);
    for frontmatter in [
        "name: docs-demo\ndescription: Demo.\nrunx:\n  category: content".to_owned(),
        "name: docs-demo\ndescription: Demo.\nregistry_owner: zhtwangk".to_owned(),
        "name: Docs-Demo\ndescription: Demo.".to_owned(),
        "name: docs--demo\ndescription: Demo.".to_owned(),
        "name: -docs-demo\ndescription: Demo.".to_owned(),
        "name: docs_demo\ndescription: Demo.".to_owned(),
        format!("name: {}\ndescription: Demo.", "a".repeat(65)),
        "name: docs-demo".to_owned(),
        format!("name: docs-demo\ndescription: {long_description}"),
        format!("name: docs-demo\ndescription: Demo.\ncompatibility: {long_compatibility}"),
        "name: docs-demo\ndescription: Demo.\nmetadata:\n  count: 3".to_owned(),
        "name: docs-demo\ndescription: Demo.\nallowed-tools: [Read]".to_owned(),
    ] {
        let raw = parse_skill_markdown(&format!("---\n{frontmatter}\n---\n# Demo\n"))
            .map_err(|error| error.to_string())?;
        assert!(
            validate_skill(raw).is_err(),
            "strict validation should reject: {frontmatter}"
        );
    }
    Ok(())
}

#[test]
fn runner_manifest_fixtures_match_expected() -> Result<(), String> {
    for fixture_json in RUNNER_MANIFEST_FIXTURES {
        let fixture: RunnerManifestFixture =
            serde_json::from_str(fixture_json).map_err(|error| error.to_string())?;
        assert_runner_manifest_fixture(fixture)?;
    }
    Ok(())
}

#[test]
fn tool_manifest_fixtures_match_expected() -> Result<(), String> {
    for fixture_json in TOOL_MANIFEST_FIXTURES {
        let fixture: ToolManifestFixture =
            serde_json::from_str(fixture_json).map_err(|error| error.to_string())?;
        assert_tool_manifest_fixture(fixture)?;
    }
    Ok(())
}

#[test]
fn install_fixtures_match_expected() -> Result<(), String> {
    for fixture_json in INSTALL_FIXTURES {
        let fixture: InstallFixture =
            serde_json::from_str(fixture_json).map_err(|error| error.to_string())?;
        assert_install_fixture(fixture)?;
    }
    Ok(())
}

fn assert_graph_fixture(fixture: GraphFixture) -> Result<(), String> {
    if let Some(expected) = fixture.expected.validated {
        let actual = validate_graph(
            parse_graph_yaml(&fixture.input.yaml).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        assert_json_eq(actual, expected)?;
        return Ok(());
    }

    let rejection = fixture
        .expected
        .rejection
        .ok_or_else(|| "fixture must declare validated or rejection".to_owned())?;
    assert_graph_rejection(&fixture.input.yaml, rejection)
}

fn assert_skill_fixture(fixture: SkillFixture) -> Result<(), String> {
    if let Some(expected) = fixture.expected.validated {
        let actual = validate_skill(
            parse_skill_markdown(&fixture.input.markdown).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        assert_json_eq(actual, expected)?;
        return Ok(());
    }

    let rejection = fixture
        .expected
        .rejection
        .ok_or_else(|| "fixture must declare validated or rejection".to_owned())?;
    assert_skill_rejection(&fixture.input.markdown, rejection)
}

fn assert_runner_manifest_fixture(fixture: RunnerManifestFixture) -> Result<(), String> {
    if let Some(expected) = fixture.expected.validated {
        let actual = validate_runner_manifest(
            parse_runner_manifest_yaml(&fixture.input.yaml).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        assert_json_eq(actual, expected)?;
        return Ok(());
    }

    let rejection = fixture
        .expected
        .rejection
        .ok_or_else(|| "fixture must declare validated or rejection".to_owned())?;
    assert_runner_manifest_rejection(&fixture.input.yaml, rejection)
}

fn assert_tool_manifest_fixture(fixture: ToolManifestFixture) -> Result<(), String> {
    if let Some(expected) = fixture.expected.validated {
        let actual = validate_tool_manifest(parse_tool_manifest_input(&fixture.input)?)
            .map_err(|error| error.to_string())?;
        assert_json_eq(actual, expected)?;
        return Ok(());
    }

    let rejection = fixture
        .expected
        .rejection
        .ok_or_else(|| "fixture must declare validated or rejection".to_owned())?;
    assert_tool_manifest_rejection(fixture.input, rejection)
}

fn assert_install_fixture(fixture: InstallFixture) -> Result<(), String> {
    let expected = fixture
        .expected
        .validated
        .ok_or_else(|| "install fixture must declare validated".to_owned())?;
    let actual = validate_skill_install(&fixture.input.markdown, fixture.input.origin)
        .map_err(|error| error.to_string())?;
    assert_json_eq(actual, expected)
}

fn assert_graph_rejection(yaml: &str, rejection: Rejection) -> Result<(), String> {
    match rejection.kind {
        RejectionKind::Parse => {
            assert!(parse_graph_yaml(yaml).is_err());
            Ok(())
        }
        RejectionKind::Validation => {
            let raw = parse_graph_yaml(yaml).map_err(|error| error.to_string())?;
            let error = validate_graph(raw).err().ok_or_else(|| {
                format!(
                    "graph fixture unexpectedly passed; expected validation error: {}",
                    rejection.message
                )
            })?;
            assert_eq!(error.to_string(), rejection.message);
            Ok(())
        }
    }
}

fn assert_skill_rejection(markdown: &str, rejection: Rejection) -> Result<(), String> {
    match rejection.kind {
        RejectionKind::Parse => {
            assert!(parse_skill_markdown(markdown).is_err());
            Ok(())
        }
        RejectionKind::Validation => {
            let raw = parse_skill_markdown(markdown).map_err(|error| error.to_string())?;
            let error = validate_skill(raw).err().ok_or_else(|| {
                format!(
                    "skill fixture unexpectedly passed; expected validation error: {}",
                    rejection.message
                )
            })?;
            assert_eq!(error.to_string(), rejection.message);
            Ok(())
        }
    }
}

fn assert_runner_manifest_rejection(yaml: &str, rejection: Rejection) -> Result<(), String> {
    match rejection.kind {
        RejectionKind::Parse => {
            assert!(parse_runner_manifest_yaml(yaml).is_err());
            Ok(())
        }
        RejectionKind::Validation => {
            let raw = parse_runner_manifest_yaml(yaml).map_err(|error| error.to_string())?;
            let error = validate_runner_manifest(raw).err().ok_or_else(|| {
                format!(
                    "runner manifest fixture unexpectedly passed; expected validation error: {}",
                    rejection.message
                )
            })?;
            assert_eq!(error.to_string(), rejection.message);
            Ok(())
        }
    }
}

fn assert_tool_manifest_rejection(input: ToolInput, rejection: Rejection) -> Result<(), String> {
    match rejection.kind {
        RejectionKind::Parse => {
            assert!(parse_tool_manifest_input(&input).is_err());
            Ok(())
        }
        RejectionKind::Validation => {
            let raw = parse_tool_manifest_input(&input)?;
            let error = validate_tool_manifest(raw).err().ok_or_else(|| {
                format!(
                    "tool manifest fixture unexpectedly passed; expected validation error: {}",
                    rejection.message
                )
            })?;
            assert_eq!(error.to_string(), rejection.message);
            Ok(())
        }
    }
}

fn parse_tool_manifest_input(input: &ToolInput) -> Result<runx_parser::RawToolManifestIr, String> {
    if let Some(yaml) = &input.yaml {
        return parse_tool_manifest_yaml(yaml).map_err(|error| error.to_string());
    }
    if let Some(json) = &input.json {
        return parse_tool_manifest_json(json).map_err(|error| error.to_string());
    }
    Err("tool fixture must declare yaml or json input".to_owned())
}

fn assert_json_eq<T>(actual: T, expected: T) -> Result<(), String>
where
    T: serde::Serialize,
{
    let actual_json = serde_json::to_value(actual).map_err(|error| error.to_string())?;
    let expected_json = serde_json::to_value(expected).map_err(|error| error.to_string())?;
    assert_eq!(actual_json, expected_json);
    Ok(())
}
