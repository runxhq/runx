use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use runx_cli::export::{ExportError, ExportPlan, Target, run_export_command};

use crate::support::temp_root;

#[test]
fn exports_public_skills_to_claude_global_with_absolute_delegation()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = ExportFixture::new("runx-export-claude-global")?;
    fixture.write_skill("visible", None)?;
    fixture.write_skill("hidden", Some("internal"))?;

    let report = run_export_command(
        &ExportPlan {
            target: Target::Claude,
            refs: Vec::new(),
            project: false,
            json: false,
        },
        &fixture.project,
        &fixture.env,
    )?;

    assert_eq!(report.exported.len(), 1);
    assert_eq!(report.exported[0].skill, "visible");
    let shim = fixture.read_home_file(".claude/skills/visible/SKILL.md")?;
    assert!(shim.contains(
        "allowed-tools: Bash(/opt/runx/bin/runx skill *) Bash(/opt/runx/bin/runx resume *)"
    ));
    assert!(!shim.contains("allowed-tools: Bash(/opt/runx/bin/runx *)"));
    assert!(shim.contains("/opt/runx/bin/runx skill"));
    assert!(
        shim.contains(
            fixture
                .project
                .join("skills/visible")
                .to_str()
                .unwrap_or_default()
        )
    );
    assert!(shim.contains("runx-export:claude"));
    assert!(!fixture.home.join(".claude/skills/hidden/SKILL.md").exists());
    Ok(())
}

#[test]
fn exports_claude_project_scope_with_project_relative_skill_ref()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = ExportFixture::new("runx-export-claude-project")?;
    fixture.write_skill("visible", None)?;

    run_export_command(
        &ExportPlan {
            target: Target::Claude,
            refs: Vec::new(),
            project: true,
            json: false,
        },
        &fixture.project,
        &fixture.env,
    )?;

    let shim = fixture.read_project_file(".claude/skills/visible/SKILL.md")?;
    assert!(shim.contains("/opt/runx/bin/runx skill skills/visible"));
    assert!(shim.contains(r#"--input-json objective '"fixture"'"#));
    assert!(!shim.contains(&format!(
        "runx skill {}",
        fixture.project.to_str().unwrap_or_default()
    )));
    Ok(())
}

#[test]
fn explicit_ref_rejects_internal_skill() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = ExportFixture::new("runx-export-explicit-internal")?;
    fixture.write_skill("hidden", Some("internal"))?;

    let error = match run_export_command(
        &ExportPlan {
            target: Target::Claude,
            refs: vec!["hidden".to_owned()],
            project: false,
            json: false,
        },
        &fixture.project,
        &fixture.env,
    ) {
        Ok(_) => return Err("internal skills were exported into agent discovery".into()),
        Err(error) => error,
    };

    assert!(
        error
            .to_string()
            .contains("cannot export hidden as an agent skill")
    );
    assert!(!fixture.home.join(".claude/skills/hidden/SKILL.md").exists());
    Ok(())
}

#[test]
fn codex_global_writes_shim_and_idempotent_permission_block()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = ExportFixture::new("runx-export-codex")?;
    fixture.write_skill("visible", None)?;
    fs::create_dir_all(fixture.home.join(".codex/rules"))?;
    fs::write(
        fixture.home.join(".codex/rules/default.rules"),
        "# existing approval\nallow_rule(pattern = [\"git\", \"status\"])\n",
    )?;

    run_export_command(
        &ExportPlan {
            target: Target::Codex,
            refs: Vec::new(),
            project: false,
            json: false,
        },
        &fixture.project,
        &fixture.env,
    )?;
    run_export_command(
        &ExportPlan {
            target: Target::Codex,
            refs: Vec::new(),
            project: false,
            json: false,
        },
        &fixture.project,
        &fixture.env,
    )?;

    let shim = fixture.read_home_file(".codex/skills/visible/SKILL.md")?;
    assert!(shim.contains("name: visible"));
    assert!(!shim.contains("allowed-tools"));
    assert!(shim.contains(r#"--input-json objective '"fixture"'"#));
    assert!(shim.contains("local-development receipt identity"));
    assert!(shim.contains("complete signer tuple"));
    assert!(shim.contains("If runx returns `needs_agent` or `needs_approval`"));
    assert!(shim.contains("digest-bound request artifact"));
    assert!(shim.contains("allowed_tools"));
    assert!(shim.contains("Answer each request by the id the run lists"));
    assert!(shim.contains("resume \"<run_id>\" - --package-digest sha256:"));
    assert!(shim.contains("--execution-closure-digest sha256:"));
    assert!(shim.contains("runx-export:codex"));
    let rules = fixture.read_home_file(".codex/rules/default.rules")?;
    assert!(rules.contains("# existing approval"));
    assert_eq!(rules.matches("runx-export start").count(), 1);
    assert_eq!(
        rules
            .matches("prefix_rule(pattern = [\"runx\", \"skill\"]")
            .count(),
        1
    );
    assert_eq!(
        rules
            .matches("prefix_rule(pattern = [\"runx\", \"resume\"]")
            .count(),
        1
    );
    assert_eq!(
        rules
            .matches("prefix_rule(pattern = [\"/opt/runx/bin/runx\", \"skill\"]")
            .count(),
        1
    );
    assert_eq!(
        rules
            .matches("prefix_rule(pattern = [\"/opt/runx/bin/runx\", \"resume\"]")
            .count(),
        1
    );
    Ok(())
}

#[test]
fn codex_exports_runtime_self_skill_as_native_instructions()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = ExportFixture::new("runx-export-native-runtime-guide")?;
    fs::write(
        fixture.project.join("SKILL.md"),
        r#"---
name: runx
description: Use the Runx runtime.
---
# Runx operator guide

Choose the smallest governed skill and invoke it with `runx skill`.
"#,
    )?;

    run_export_command(
        &ExportPlan {
            target: Target::Codex,
            refs: Vec::new(),
            project: false,
            json: false,
        },
        &fixture.project,
        &fixture.env,
    )?;

    let shim = fixture.read_home_file(".codex/skills/runx/SKILL.md")?;
    assert!(shim.contains("# Runx operator guide"));
    assert!(shim.contains("Choose the smallest governed skill"));
    assert!(shim.contains("runx-export:codex"));
    assert!(!shim.contains("/opt/runx/bin/runx skill"));
    assert!(!shim.contains("--prompt \"<prompt>\""));
    Ok(())
}

#[test]
fn codex_global_initializes_missing_codex_home() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = ExportFixture::new("runx-export-codex-first-run")?;
    fixture.write_skill("visible", None)?;

    let report = run_export_command(
        &ExportPlan {
            target: Target::Codex,
            refs: Vec::new(),
            project: false,
            json: false,
        },
        &fixture.project,
        &fixture.env,
    )?;

    assert_eq!(report.exported.len(), 1);
    assert_eq!(report.exported[0].skill, "visible");
    assert!(fixture.home.join(".codex/skills/visible/SKILL.md").exists());
    assert!(fixture.home.join(".codex/rules/default.rules").exists());
    let rules = fixture.read_home_file(".codex/rules/default.rules")?;
    assert!(rules.contains("prefix_rule(pattern = [\"runx\", \"skill\"]"));
    assert!(rules.contains("prefix_rule(pattern = [\"runx\", \"resume\"]"));
    assert!(rules.contains("prefix_rule(pattern = [\"/opt/runx/bin/runx\", \"skill\"]"));
    assert!(rules.contains("prefix_rule(pattern = [\"/opt/runx/bin/runx\", \"resume\"]"));
    Ok(())
}

#[test]
fn exports_default_runner_inputs_when_skill_frontmatter_has_none()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = ExportFixture::new("runx-export-runner-inputs")?;
    fixture.write_skill_with_runner_inputs("work-plan")?;

    run_export_command(
        &ExportPlan {
            target: Target::Codex,
            refs: Vec::new(),
            project: false,
            json: false,
        },
        &fixture.project,
        &fixture.env,
    )?;

    let shim = fixture.read_home_file(".codex/skills/work-plan/SKILL.md")?;
    assert!(shim.contains(r#"--input-json objective '"fixture"'"#));
    assert!(shim.contains(r#"--input-json principal '"fixture"'"#));
    assert!(!shim.contains("\"input_schema\""));
    assert!(!shim.contains("\"required\": ["));
    assert!(shim.contains("skill inspect"));
    assert!(
        shim.contains(
            "Report an external mutation only when the result contains provider evidence"
        )
    );
    assert!(shim.contains("--package-digest sha256:"));
    assert!(shim.contains("--execution-closure-digest sha256:"));
    assert!(!shim.contains("Do not perform the work yourself"));
    Ok(())
}

#[test]
fn omits_multi_runner_skill_without_default() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = ExportFixture::new("runx-export-multi-runner-selection")?;
    fixture.write_multi_runner_skill_without_default("acme-operator")?;

    let report = run_export_command(
        &ExportPlan {
            target: Target::Codex,
            refs: Vec::new(),
            project: false,
            json: false,
        },
        &fixture.project,
        &fixture.env,
    )?;

    assert!(report.exported.is_empty());
    assert!(
        !fixture
            .home
            .join(".codex/skills/acme-operator/SKILL.md")
            .exists()
    );
    Ok(())
}

#[test]
fn generated_shims_are_compact_digest_bound_and_detect_drift()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = ExportFixture::new("runx-export-digest-bound")?;
    fixture.write_skill("visible", None)?;

    run_export_command(
        &ExportPlan {
            target: Target::Codex,
            refs: Vec::new(),
            project: false,
            json: false,
        },
        &fixture.project,
        &fixture.env,
    )?;

    let shim = fixture.read_home_file(".codex/skills/visible/SKILL.md")?;
    assert!(
        shim.len() < 16 * 1024,
        "compact shim was {} bytes",
        shim.len()
    );
    assert_eq!(shim.matches("--package-digest sha256:").count(), 2);
    assert_eq!(
        shim.matches("--execution-closure-digest sha256:").count(),
        2
    );
    assert!(!shim.contains("Input contract:"));

    let package_digest = flag_value(&shim, "--package-digest")?;
    let closure_digest = flag_value(&shim, "--execution-closure-digest")?;
    let manual = fixture.project.join("skills/visible/SKILL.md");
    fs::write(
        &manual,
        "---\nname: visible\ndescription: Export visible through runx after a source change.\n---\n# visible\n\nChanged instructions.\n",
    )?;

    let output = crate::support::unsigned_runx_command_at(&fixture.project)
        .args([
            "skill",
            fixture
                .project
                .join("skills/visible")
                .to_str()
                .ok_or("skill path")?,
            "default",
            "--objective",
            "prove stale export refusal",
            "--package-digest",
            package_digest,
            "--execution-closure-digest",
            closure_digest,
            "--json",
        ])
        .output()?;
    assert!(!output.status.success());
    let error = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(error.contains("package digest mismatch"), "{error}");

    let report = run_export_command(
        &ExportPlan {
            target: Target::Codex,
            refs: Vec::new(),
            project: false,
            json: false,
        },
        &fixture.project,
        &fixture.env,
    )?;
    assert_eq!(report.warnings.len(), 1);
    assert!(report.warnings[0].contains("replaced stale managed export"));
    Ok(())
}

fn flag_value<'a>(contents: &'a str, flag: &str) -> Result<&'a str, Box<dyn std::error::Error>> {
    contents
        .split_once(&format!("{flag} "))
        .and_then(|(_, suffix)| suffix.split_whitespace().next())
        .ok_or_else(|| format!("missing {flag} in generated shim").into())
}

#[test]
fn explicit_ref_exports_official_source_skill() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = ExportFixture::new("runx-export-official-ref")?;
    let official_root = fixture.write_official_skill_with_runner_inputs("work-plan")?;
    let mut env = fixture.env.clone();
    env.insert(
        "RUNX_OFFICIAL_SKILLS_SOURCE_DIR".to_owned(),
        path_string(&official_root)?,
    );

    let report = run_export_command(
        &ExportPlan {
            target: Target::Codex,
            refs: vec!["work-plan".to_owned()],
            project: false,
            json: false,
        },
        &fixture.project,
        &env,
    )?;

    assert_eq!(report.exported.len(), 1);
    assert_eq!(report.exported[0].skill, "work-plan");
    let shim = fixture.read_home_file(".codex/skills/work-plan/SKILL.md")?;
    assert!(shim.contains("skill "));
    assert!(shim.contains("/official-skills/work-plan"));
    assert!(shim.contains(r#"--input-json objective '"fixture"'"#));
    Ok(())
}

#[test]
fn reexport_prunes_only_marked_generated_files() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = ExportFixture::new("runx-export-prune")?;
    fixture.write_skill("visible", None)?;
    let managed = fixture.home.join(".claude/skills/stale/SKILL.md");
    let foreign = fixture.home.join(".claude/skills/foreign/SKILL.md");
    let manual = fixture.home.join(".claude/skills/manual/SKILL.md");
    fs::create_dir_all(managed.parent().ok_or("managed parent")?)?;
    fs::create_dir_all(foreign.parent().ok_or("foreign parent")?)?;
    fs::create_dir_all(manual.parent().ok_or("manual parent")?)?;
    fs::write(
        &managed,
        format!(
            "---\nname: stale\ndescription: stale fixture.\n---\n<!-- runx-export:claude source={} - generated, do not edit -->\n",
            fs::canonicalize(&fixture.project)?
                .join("skills/stale")
                .display()
        ),
    )?;
    fs::write(
        &foreign,
        "---\nname: foreign\ndescription: foreign fixture.\n---\n<!-- runx-export:claude source=/another/workspace/skills/foreign - generated, do not edit -->\n",
    )?;
    fs::write(
        &manual,
        "---\nname: manual\ndescription: manual fixture.\n---\n# Hand-authored\n",
    )?;

    let report = run_export_command(
        &ExportPlan {
            target: Target::Claude,
            refs: Vec::new(),
            project: false,
            json: false,
        },
        &fixture.project,
        &fixture.env,
    )?;

    assert_eq!(report.pruned.len(), 1);
    assert!(!managed.exists());
    assert!(foreign.exists());
    assert!(manual.exists());
    Ok(())
}

#[test]
fn full_export_outside_a_skill_workspace_fails_before_pruning()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = ExportFixture::new("runx-export-invalid-root")?;
    let unrelated = fixture.project.join("unrelated");
    let managed = fixture.home.join(".codex/skills/existing/SKILL.md");
    fs::create_dir_all(&unrelated)?;
    fs::create_dir_all(managed.parent().ok_or("managed parent")?)?;
    fs::write(
        &managed,
        format!(
            "---\nname: existing\ndescription: existing fixture.\n---\n<!-- runx-export:codex source={} - generated, do not edit -->\n",
            fixture.project.join("skills/existing").display()
        ),
    )?;

    let Err(error) = run_export_command(
        &ExportPlan {
            target: Target::Codex,
            refs: Vec::new(),
            project: false,
            json: false,
        },
        &unrelated,
        &fixture.env,
    ) else {
        return Err("an unrelated directory must not become a destructive full export".into());
    };

    match error {
        ExportError::InvalidArgs(message) => {
            assert!(message.contains("is not a skill workspace"));
        }
        other => return Err(format!("unexpected error: {other}").into()),
    }
    assert!(managed.exists());
    Ok(())
}

#[test]
fn export_refuses_to_overwrite_an_unmanaged_skill() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = ExportFixture::new("runx-export-unmanaged-collision")?;
    fixture.write_skill("visible", None)?;
    let manual = fixture.home.join(".codex/skills/visible/SKILL.md");
    fs::create_dir_all(manual.parent().ok_or("manual parent")?)?;
    fs::write(
        &manual,
        "---\nname: visible\ndescription: visible fixture.\n---\n# Hand-authored\n",
    )?;

    let Err(error) = run_export_command(
        &ExportPlan {
            target: Target::Codex,
            refs: Vec::new(),
            project: false,
            json: false,
        },
        &fixture.project,
        &fixture.env,
    ) else {
        return Err("an unmanaged skill collision must fail closed".into());
    };

    match error {
        ExportError::InvalidArgs(message) => {
            assert!(message.contains("is not managed by this export"));
        }
        other => return Err(format!("unexpected error: {other}").into()),
    }
    assert_eq!(
        fs::read_to_string(manual)?,
        "---\nname: visible\ndescription: visible fixture.\n---\n# Hand-authored\n"
    );
    Ok(())
}

#[test]
fn explicit_ref_export_preserves_other_managed_skills() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = ExportFixture::new("runx-export-explicit-preserves")?;
    fixture.write_skill("first", None)?;
    fixture.write_skill("second", None)?;

    run_export_command(
        &ExportPlan {
            target: Target::Codex,
            refs: Vec::new(),
            project: false,
            json: false,
        },
        &fixture.project,
        &fixture.env,
    )?;

    let report = run_export_command(
        &ExportPlan {
            target: Target::Codex,
            refs: vec!["first".to_owned()],
            project: false,
            json: false,
        },
        &fixture.project,
        &fixture.env,
    )?;

    assert!(report.pruned.is_empty());
    assert!(fixture.home.join(".codex/skills/first/SKILL.md").exists());
    assert!(fixture.home.join(".codex/skills/second/SKILL.md").exists());
    Ok(())
}

#[test]
fn codex_project_scope_fails_closed() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = ExportFixture::new("runx-export-codex-project")?;
    fixture.write_skill("visible", None)?;

    let error = match run_export_command(
        &ExportPlan {
            target: Target::Codex,
            refs: Vec::new(),
            project: true,
            json: false,
        },
        &fixture.project,
        &fixture.env,
    ) {
        Ok(_) => return Err("codex project export should be disabled".into()),
        Err(error) => error,
    };

    match error {
        ExportError::Unsupported(message) => {
            assert!(message.contains("not supported until Codex project skill"));
        }
        other => return Err(format!("unexpected error: {other}").into()),
    }
    Ok(())
}

#[test]
fn rejects_skill_names_that_escape_export_directory() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = ExportFixture::new("runx-export-path-traversal")?;
    let dir = fixture.project.join("skills").join("bad");
    fs::create_dir_all(&dir)?;
    fs::write(
        dir.join("SKILL.md"),
        "---\nname: ../outside\ndescription: Escape attempt.\n---\n# bad\n",
    )?;

    let error = match run_export_command(
        &ExportPlan {
            target: Target::Claude,
            refs: Vec::new(),
            project: false,
            json: false,
        },
        &fixture.project,
        &fixture.env,
    ) {
        Ok(_) => return Err("unsafe skill name should fail".into()),
        Err(error) => error,
    };

    assert!(
        error.to_string().contains("name '../outside' must be"),
        "unexpected error: {error}"
    );
    assert!(!fixture.home.join(".claude/outside/SKILL.md").exists());
    Ok(())
}

#[test]
fn rejects_input_names_that_are_not_safe_shell_flags() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = ExportFixture::new("runx-export-unsafe-input")?;
    fixture.write_skill_with_input("bad", "bad$name")?;

    let error = match run_export_command(
        &ExportPlan {
            target: Target::Claude,
            refs: Vec::new(),
            project: false,
            json: false,
        },
        &fixture.project,
        &fixture.env,
    ) {
        Ok(_) => return Err("unsafe input name should fail".into()),
        Err(error) => error,
    };

    match error {
        ExportError::InvalidArgs(message) => {
            assert!(message.contains("not a safe runx skill flag"));
        }
        other => return Err(format!("unexpected error: {other}").into()),
    }
    Ok(())
}

#[test]
fn rejects_input_names_that_collide_with_runx_skill_flags() -> Result<(), Box<dyn std::error::Error>>
{
    let fixture = ExportFixture::new("runx-export-reserved-input")?;
    fixture.write_skill_with_input("bad", "json")?;

    let error = match run_export_command(
        &ExportPlan {
            target: Target::Claude,
            refs: Vec::new(),
            project: false,
            json: false,
        },
        &fixture.project,
        &fixture.env,
    ) {
        Ok(_) => return Err("reserved input name should fail".into()),
        Err(error) => error,
    };

    match error {
        ExportError::InvalidArgs(message) => {
            assert!(message.contains("not a safe runx skill flag"));
        }
        other => return Err(format!("unexpected error: {other}").into()),
    }
    Ok(())
}

struct ExportFixture {
    root: PathBuf,
    project: PathBuf,
    home: PathBuf,
    env: BTreeMap<String, String>,
}

impl ExportFixture {
    fn new(prefix: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let root = temp_root(prefix);
        let project = root.join("project");
        let home = root.join("home");
        fs::create_dir_all(&project)?;
        fs::create_dir_all(&home)?;
        let env = [
            ("HOME".to_owned(), path_string(&home)?),
            (
                "RUNX_EXPORT_BIN".to_owned(),
                "/opt/runx/bin/runx".to_owned(),
            ),
        ]
        .into_iter()
        .collect();
        Ok(Self {
            root,
            project,
            home,
            env,
        })
    }

    fn write_skill(
        &self,
        name: &str,
        visibility: Option<&str>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.write_skill_with_input_and_visibility(name, "objective", visibility)
    }

    fn write_skill_with_input(
        &self,
        name: &str,
        input_name: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.write_skill_with_input_and_visibility(name, input_name, None)
    }

    fn write_skill_with_input_and_visibility(
        &self,
        name: &str,
        input_name: &str,
        visibility: Option<&str>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let dir = self.project.join("skills").join(name);
        let harness = readiness_harness("default", &[input_name]);
        fs::create_dir_all(&dir)?;
        fs::write(
            dir.join("SKILL.md"),
            format!(
                "---\nname: {name}\ndescription: Export {name} through runx.\n---\n# {name}\n\nRun the governed skill.\n"
            ),
        )?;
        fs::write(
            dir.join("run.yaml"),
            format!(
                "skill: {name}\ncatalog:\n  kind: skill\n  audience: public\n  visibility: {}\n  role: context\n{harness}runners:\n  default:\n    default: true\n    inputs:\n      {input_name}:\n        type: string\n        required: true\n        description: Work to perform.\n    steps:\n      - id: default\n        agent: {{ role: reviewer, task: {name} }}\n        outputs:\n          result: object\n",
                visibility.unwrap_or("public")
            ),
        )?;
        Ok(())
    }

    fn write_skill_with_runner_inputs(&self, name: &str) -> Result<(), Box<dyn std::error::Error>> {
        let dir = self.project.join("skills").join(name);
        Self::write_runner_input_skill_at(&dir, name)
    }

    fn write_multi_runner_skill_without_default(
        &self,
        name: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let dir = self.project.join("skills").join(name);
        fs::create_dir_all(&dir)?;
        fs::write(
            dir.join("SKILL.md"),
            format!(
                "---\nname: {name}\ndescription: Operate one bounded board action.\n---\n# {name}\n"
            ),
        )?;
        fs::write(
            dir.join("run.yaml"),
            format!(
                "skill: {name}\ncatalog:\n  kind: skill\n  audience: operator\n  visibility: public\n  role: canonical\nrunners:\n  payments:\n    result_from: [read]\n    steps:\n      - id: read\n        tool: data.compare\n  payout_preflight:\n    inputs:\n      claim:\n        type: string\n        required: true\n    result_from: [read]\n    steps:\n      - id: read\n        tool: data.compare\n"
            ),
        )?;
        Ok(())
    }

    fn write_official_skill_with_runner_inputs(
        &self,
        name: &str,
    ) -> Result<PathBuf, Box<dyn std::error::Error>> {
        let root = self.root.join("official-skills");
        Self::write_runner_input_skill_at(&root.join(name), name)?;
        Ok(root)
    }

    fn write_runner_input_skill_at(
        dir: &Path,
        name: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let harness = readiness_harness("plan", &["objective", "principal"]);
        fs::create_dir_all(dir)?;
        fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: Export {name} through runx.\n---\n# {name}\n"),
        )?;
        fs::write(
            dir.join("run.yaml"),
            format!(
                "skill: {name}\ncatalog:\n  kind: skill\n  audience: public\n  visibility: public\n  role: canonical\n{harness}runners:\n  plan:\n    default: true\n    inputs:\n      objective:\n        type: string\n        required: true\n        description: Bounded send objective.\n      principal:\n        type: string\n        required: true\n        description: Principal represented by the send.\n      provider_context:\n        type: json\n        required: false\n        description: Provider readiness.\n    steps:\n      - id: plan\n        agent: {{ role: reviewer, task: {name} }}\n        outputs:\n          result: object\n"
            ),
        )?;
        Ok(())
    }

    fn read_home_file(&self, relative: &str) -> Result<String, Box<dyn std::error::Error>> {
        Ok(fs::read_to_string(self.home.join(relative))?)
    }

    fn read_project_file(&self, relative: &str) -> Result<String, Box<dyn std::error::Error>> {
        Ok(fs::read_to_string(self.project.join(relative))?)
    }
}

fn readiness_harness(runner: &str, inputs: &[&str]) -> String {
    let inputs = inputs
        .iter()
        .map(|input| format!("        \"{input}\": fixture\n"))
        .collect::<String>();
    format!(
        "harness:\n  cases:\n    - name: export-operator-journeys\n      runner: {runner}\n      operator_journeys:\n        - mode: standalone\n          confusors: [research, issue-intake, extract]\n          request: Run the advertised default operation from a cold selection.\n          expected_outcome: Return the promised bounded result.\n        - mode: composed\n          request: Continue from a prior bounded result.\n          expected_outcome: Reuse supplied evidence without repeating completed work.\n          prior_evidence:\n            - A typed result supplied by the prior step.\n          must_not_repeat:\n            - Do not repeat prior discovery.\n      inputs:\n{inputs}      expect:\n        status: sealed\n"
    )
}

impl Drop for ExportFixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).ok();
    }
}

fn path_string(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    path.to_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| "path is not UTF-8".into())
}
