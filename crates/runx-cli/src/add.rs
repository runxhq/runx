//! Repository indexing CLI: thin orchestrator over the runtime's `/v1/index`
//! client. This module owns argument resolution (env + plan defaults) and
//! presentation (human text vs JSON envelope); it owns no parsing, HTTP, or
//! response shaping logic — those live in `runx_runtime::registry::index`.

use std::collections::BTreeMap;
use std::process::ExitCode;

use runx_runtime::WorkspaceEnv;
use runx_runtime::registry::{
    IndexRepositoryOptions, IndexResponse, IndexWarning, IndexedListing, IndexedRepo, TrustTier,
    index_repository,
};
use serde::Serialize;

use crate::router::AddUrlPlan;

pub fn run_native_add(plan: AddUrlPlan, workspace: &WorkspaceEnv) -> ExitCode {
    let environment = match resolve_public_api_environment(&plan, workspace.env(), workspace.cwd())
    {
        Ok(environment) => environment,
        Err(error) => return fail(&error.to_string()),
    };

    let repo_url = plan.repo.trim();
    let transport = match runx_runtime::hosted_api_transport(
        runx_runtime::hosted_private_network_allowed(false, workspace.env()),
    ) {
        Ok(transport) => transport,
        Err(error) => return fail(&format!("failed to initialize HTTP transport: {error}")),
    };

    let options = IndexRepositoryOptions {
        base_url: environment.base_url(),
        repo_url,
        repo_ref: plan.repo_ref.as_deref(),
    };

    match index_repository(&transport, &options) {
        Ok(response) => render_result(plan.json, repo_url, &response),
        Err(error) => fail(&error.to_string()),
    }
}

fn resolve_public_api_environment(
    plan: &AddUrlPlan,
    env: &BTreeMap<String, String>,
    cwd: &std::path::Path,
) -> Result<runx_runtime::HostedApiEnvironment, runx_runtime::HostedApiError> {
    runx_runtime::HostedApiEnvironment::resolve_unauthenticated(
        plan.api_base_url.as_deref(),
        env,
        cwd,
    )
}

fn render_result(json: bool, repo_url: &str, response: &IndexResponse) -> ExitCode {
    if json {
        let envelope = AddUrlJsonResult {
            status: "success",
            requested: AddUrlRequestedRef { repo_url },
            repo: &response.repo,
            listings: &response.listings,
            warnings: &response.warnings,
        };
        match serde_json::to_string_pretty(&envelope) {
            Ok(serialized) => crate::cli_io::write_stdout_code(&format!("{serialized}\n"), 0),
            Err(error) => fail(&format!("failed to serialize add result: {error}")),
        }
    } else {
        crate::cli_io::write_stdout_code(&render_text(response), 0)
    }
}

fn render_text(response: &IndexResponse) -> String {
    let mut out = String::new();
    let sha_short: String = response.repo.sha.chars().take(12).collect();
    let count = response.listings.len();
    out.push_str(&format!(
        "indexed {count} skill{plural} from {owner}/{repo}@{sha}\n\n",
        plural = if count == 1 { "" } else { "s" },
        owner = response.repo.owner,
        repo = response.repo.repo,
        sha = sha_short,
    ));
    for listing in &response.listings {
        let tag = if listing.digest_unchanged {
            "(unchanged)"
        } else {
            "(new)"
        };
        out.push_str(&format!(
            "  {}@{} · {} {}\n",
            listing.skill_id,
            listing.version,
            trust_tier_label(&listing.trust_tier),
            tag,
        ));
        out.push_str(&format!("    → {}\n", listing.permalink));
        out.push_str(&format!(
            "    install: runx add {}@{}\n",
            listing.skill_id, listing.version,
        ));
        out.push_str(&format!("    run:     runx {}\n\n", listing.name));
    }
    if !response.warnings.is_empty() {
        out.push_str("warnings:\n");
        for warning in &response.warnings {
            let where_ = warning
                .skill_path
                .as_deref()
                .map(|path| format!(" ({path})"))
                .unwrap_or_default();
            out.push_str(&format!(
                "  - {}{}: {}\n",
                warning.code, where_, warning.detail,
            ));
        }
        out.push('\n');
    }
    out
}

fn trust_tier_label(tier: &TrustTier) -> &'static str {
    match tier {
        TrustTier::FirstParty => "first_party",
        TrustTier::Verified => "verified",
        TrustTier::Community => "community",
    }
}

fn fail(message: &str) -> ExitCode {
    let _ignored = crate::cli_io::write_stderr(&format!("runx: {message}\n"));
    ExitCode::from(1)
}

#[derive(Serialize)]
struct AddUrlJsonResult<'a> {
    status: &'a str,
    requested: AddUrlRequestedRef<'a>,
    repo: &'a IndexedRepo,
    listings: &'a [IndexedListing],
    warnings: &'a [IndexWarning],
}

#[derive(Serialize)]
struct AddUrlRequestedRef<'a> {
    repo_url: &'a str,
}

#[cfg(test)]
mod tests {
    use super::*;
    use runx_runtime::registry::{IndexedRepo, TrustTier};

    fn sample_listing(skill_id: &str, version: &str, unchanged: bool) -> IndexedListing {
        IndexedListing {
            owner: skill_id.split('/').next().unwrap_or("runxhq").to_owned(),
            name: skill_id.split('/').nth(1).unwrap_or("demo").to_owned(),
            skill_id: skill_id.to_owned(),
            version: version.to_owned(),
            permalink: format!("https://runx.ai/{skill_id}/{version}"),
            trust_tier: TrustTier::Community,
            skill_path: format!("skills/{skill_id}/SKILL.md"),
            digest_unchanged: unchanged,
        }
    }

    fn sample_response(
        listings: Vec<IndexedListing>,
        warnings: Vec<IndexWarning>,
    ) -> IndexResponse {
        IndexResponse {
            repo: IndexedRepo {
                owner: "runxhq".to_owned(),
                repo: "runx".to_owned(),
                git_ref: "main".to_owned(),
                sha: "abcdef0123456789".to_owned(),
            },
            listings,
            warnings,
        }
    }

    #[test]
    fn renders_singular_for_one_listing() {
        let response = sample_response(vec![sample_listing("runxhq/demo", "sha-1", false)], vec![]);
        let text = render_text(&response);
        assert!(text.starts_with("indexed 1 skill from runxhq/runx@abcdef012345\n"));
        assert!(text.contains("(new)"));
        assert!(text.contains("install: runx add runxhq/demo@sha-1"));
    }

    #[test]
    fn renders_plural_and_unchanged_tag() {
        let response = sample_response(
            vec![
                sample_listing("runxhq/a", "sha-1", true),
                sample_listing("runxhq/b", "sha-2", false),
            ],
            vec![],
        );
        let text = render_text(&response);
        assert!(text.starts_with("indexed 2 skills from runxhq/runx@abcdef012345\n"));
        assert!(text.contains("(unchanged)"));
        assert!(text.contains("(new)"));
    }

    #[test]
    fn renders_warnings_block_only_when_present() {
        let bare = sample_response(vec![], vec![]);
        assert!(!render_text(&bare).contains("warnings:"));

        let warned = sample_response(
            vec![],
            vec![IndexWarning {
                skill_path: Some("skills/foo/SKILL.md".to_owned()),
                code: "missing_runner".to_owned(),
                detail: "runner manifest absent".to_owned(),
            }],
        );
        let text = render_text(&warned);
        assert!(text.contains("warnings:"));
        assert!(text.contains("missing_runner (skills/foo/SKILL.md): runner manifest absent"));
    }

    #[test]
    fn resolves_base_url_in_precedence_order() -> Result<(), Box<dyn std::error::Error>> {
        let plan_with_override = AddUrlPlan {
            repo: "https://github.com/runxhq/runx".to_owned(),
            repo_ref: None,
            api_base_url: Some("https://override.example/".to_owned()),
            json: false,
        };
        let mut env: BTreeMap<String, String> = BTreeMap::new();
        env.insert(
            "RUNX_PUBLIC_API_BASE_URL".to_owned(),
            "https://from-env.example/".to_owned(),
        );

        // Plan override wins, trailing slash stripped.
        let overridden =
            resolve_public_api_environment(&plan_with_override, &env, &std::env::temp_dir())?;
        assert_eq!(overridden.base_url(), "https://override.example");

        // Without plan override, env takes over.
        let plan_no_override = AddUrlPlan {
            repo: "https://github.com/runxhq/runx".to_owned(),
            repo_ref: None,
            api_base_url: None,
            json: false,
        };
        let from_env =
            resolve_public_api_environment(&plan_no_override, &env, &std::env::temp_dir())?;
        assert_eq!(from_env.base_url(), "https://from-env.example");

        // Without either, default.
        let empty_env: BTreeMap<String, String> = BTreeMap::new();
        let default =
            resolve_public_api_environment(&plan_no_override, &empty_env, &std::env::temp_dir())?;
        assert_eq!(default.base_url(), "https://api.runx.ai");
        Ok(())
    }

    #[test]
    fn trust_tier_labels_match_snake_case_wire_form() {
        assert_eq!(trust_tier_label(&TrustTier::FirstParty), "first_party");
        assert_eq!(trust_tier_label(&TrustTier::Verified), "verified");
        assert_eq!(trust_tier_label(&TrustTier::Community), "community");
    }
}
