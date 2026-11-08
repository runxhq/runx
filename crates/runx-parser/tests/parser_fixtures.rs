use serde::Deserialize;

use runx_parser::{
    SkillInstallOrigin, SkillMethodManifest, ValidatedSkill, ValidatedSkillInstall, ValidatedTool,
    Workflow, parse_method_manifest_yaml, parse_skill_markdown, parse_tool_manifest_json,
    parse_tool_manifest_yaml, parse_workflow_yaml, validate_method_manifest, validate_skill,
    validate_skill_install, validate_tool_manifest, validate_workflow,
};

const WORKFLOW_FIXTURES: &[&str] = &[
    include_str!("../../../fixtures/parser/graphs/parse-malformed-yaml.json"),
    include_str!("../../../fixtures/parser/graphs/sequential-references.json"),
    include_str!("../../../fixtures/parser/graphs/step-kinds.json"),
    include_str!("../../../fixtures/parser/graphs/tool-and-guard.json"),
    include_str!("../../../fixtures/parser/graphs/validation-missing-step-id.json"),
    include_str!("../../../fixtures/parser/graphs/validation-two-step-kinds.json"),
];

const SKILL_FIXTURES: &[&str] = &[
    include_str!("../../../fixtures/parser/skills/portable-manual.json"),
    include_str!("../../../fixtures/parser/skills/validation-manual-inputs.json"),
];

const METHOD_MANIFEST_FIXTURES: &[&str] = &[
    include_str!("../../../fixtures/parser/method-manifests/test-basic.json"),
    include_str!(
        "../../../fixtures/parser/method-manifests/validation-invalid-reflect-policy.json"
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
struct WorkflowFixture {
    input: YamlInput,
    expected: WorkflowExpected,
}

#[derive(Debug, Deserialize)]
struct SkillFixture {
    input: MarkdownInput,
    expected: SkillExpected,
}

#[derive(Debug, Deserialize)]
struct MethodManifestFixture {
    input: MethodManifestInput,
    expected: MethodManifestExpected,
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

/// A `run.yaml`, read as the package its `SKILL.md` names.
#[derive(Debug, Deserialize)]
struct MethodManifestInput {
    skill: String,
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
struct WorkflowExpected {
    validated: Option<Workflow>,
    rejection: Option<Rejection>,
}

#[derive(Debug, Deserialize)]
struct SkillExpected {
    validated: Option<ValidatedSkill>,
    rejection: Option<Rejection>,
}

#[derive(Debug, Deserialize)]
struct MethodManifestExpected {
    validated: Option<SkillMethodManifest>,
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
fn workflow_fixtures_match_expected() -> Result<(), String> {
    for fixture_json in WORKFLOW_FIXTURES {
        let fixture: WorkflowFixture =
            serde_json::from_str(fixture_json).map_err(|error| error.to_string())?;
        assert_workflow_fixture(fixture)?;
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
fn catalog_carries_category_and_tags() -> Result<(), String> {
    let manifest = validate_method_manifest(
        "docs-demo",
        parse_method_manifest_yaml(
            r#"
minimum_runx: 0.12.0
catalog:
  visibility: public
  category: content
  tags: [docs, writing]
methods:
  main:
    steps:
      - id: write
        agent: write
"#,
        )
        .map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;

    let catalog = manifest.catalog.ok_or("catalog should be present")?;
    assert_eq!(catalog.category.as_deref(), Some("content"));
    assert_eq!(catalog.tags, vec!["docs".to_owned(), "writing".to_owned()]);
    Ok(())
}

#[test]
fn catalog_rejects_repeated_tags() -> Result<(), String> {
    let raw = parse_method_manifest_yaml(
        "minimum_runx: 0.12.0\ncatalog:\n  visibility: public\n  tags: [docs, docs]\nmethods:\n  main:\n    steps:\n      - id: write\n        agent: write\n",
    )
    .map_err(|error| error.to_string())?;
    let error = validate_method_manifest("docs-demo", raw)
        .err()
        .ok_or("catalog.tags: [docs, docs] should be rejected")?;
    assert!(
        error.to_string().contains("catalog.tags repeats 'docs'"),
        "{error}"
    );
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
fn method_manifest_fixtures_match_expected() -> Result<(), String> {
    for fixture_json in METHOD_MANIFEST_FIXTURES {
        let fixture: MethodManifestFixture =
            serde_json::from_str(fixture_json).map_err(|error| error.to_string())?;
        assert_method_manifest_fixture(fixture)?;
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

fn assert_workflow_fixture(fixture: WorkflowFixture) -> Result<(), String> {
    if let Some(expected) = fixture.expected.validated {
        let actual = validate_workflow(
            parse_workflow_yaml(&fixture.input.yaml).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        assert_json_eq(actual, expected)?;
        return Ok(());
    }

    let rejection = fixture
        .expected
        .rejection
        .ok_or_else(|| "fixture must declare validated or rejection".to_owned())?;
    assert_workflow_rejection(&fixture.input.yaml, rejection)
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

fn assert_method_manifest_fixture(fixture: MethodManifestFixture) -> Result<(), String> {
    if let Some(expected) = fixture.expected.validated {
        let actual = validate_method_manifest(
            &fixture.input.skill,
            parse_method_manifest_yaml(&fixture.input.yaml).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        assert_json_eq(actual, expected)?;
        return Ok(());
    }

    let rejection = fixture
        .expected
        .rejection
        .ok_or_else(|| "fixture must declare validated or rejection".to_owned())?;
    assert_method_manifest_rejection(&fixture.input, rejection)
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

fn assert_workflow_rejection(yaml: &str, rejection: Rejection) -> Result<(), String> {
    match rejection.kind {
        RejectionKind::Parse => {
            assert!(parse_workflow_yaml(yaml).is_err());
            Ok(())
        }
        RejectionKind::Validation => {
            let raw = parse_workflow_yaml(yaml).map_err(|error| error.to_string())?;
            let error = validate_workflow(raw).err().ok_or_else(|| {
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

fn assert_method_manifest_rejection(
    input: &MethodManifestInput,
    rejection: Rejection,
) -> Result<(), String> {
    match rejection.kind {
        RejectionKind::Parse => {
            assert!(parse_method_manifest_yaml(&input.yaml).is_err());
            Ok(())
        }
        RejectionKind::Validation => {
            let raw = parse_method_manifest_yaml(&input.yaml).map_err(|error| error.to_string())?;
            let error = validate_method_manifest(&input.skill, raw).err().ok_or_else(|| {
                format!(
                    "method manifest fixture unexpectedly passed; expected validation error: {}",
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
