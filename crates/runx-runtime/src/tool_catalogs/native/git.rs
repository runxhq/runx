//! Native bounded Git observations.

use std::time::Duration;

use super::{NativeInvocation, invalid_input, resolve_repo_root_for};
use crate::RuntimeError;

mod capability;
mod history;

use crate::process::{ProcessOutcome, ProcessSpec, run_process};
use crate::process_invocation::process_base_environment;
pub(super) use capability::CAPABILITIES;
use capability::{
    GitBlobDigest, GitBlobDigestInput, GitBlobDigestOutput, GitBranchOutput, GitDiffInput,
    GitDiffOutput, GitInput, GitStatusOutput,
};

const CURRENT_BRANCH: &str = "git.current_branch";
const STATUS: &str = "git.status";
const DIFF_NAME_ONLY: &str = "git.diff_name_only";
const OUTPUT_LIMIT_BYTES: usize = 64 * 1024;
const TIMEOUT: Duration = Duration::from_secs(10);

fn blob_digest(
    invocation: &NativeInvocation<'_, GitBlobDigestInput>,
) -> Result<GitBlobDigestOutput, RuntimeError> {
    let contents = invocation.inputs.contents.as_bytes();
    let mut canonical = format!("blob {}\0", contents.len()).into_bytes();
    canonical.extend_from_slice(contents);
    let digest = ring::digest::digest(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY, &canonical);
    Ok(GitBlobDigestOutput {
        git_blob_digest: GitBlobDigest {
            algorithm: "sha1".to_owned(),
            digest: runx_contracts::hex_lower(digest.as_ref()),
            bytes: contents.len() as u64,
        },
    })
}

fn current_branch(
    invocation: &NativeInvocation<'_, GitInput>,
) -> Result<GitBranchOutput, RuntimeError> {
    let root = resolve_repo_root_for(
        CURRENT_BRANCH,
        &invocation.inputs.repo_root,
        invocation.env,
        invocation.skill_directory,
        invocation.bounds,
    )?;
    let env = git_env(invocation)?;
    let symbolic = run_git(&root, &env, &["symbolic-ref", "--short", "HEAD"])?;
    let (branch, detached) = if symbolic.status.success() {
        (output_text(CURRENT_BRANCH, invocation, symbolic)?, false)
    } else {
        let head = run_git(&root, &env, &["rev-parse", "--short", "HEAD"])?;
        if !head.status.success() {
            return Err(invalid_input(
                CURRENT_BRANCH,
                "repo_root must be a Git repository with a readable HEAD",
            ));
        }
        (output_text(CURRENT_BRANCH, invocation, head)?, true)
    };
    if branch.is_empty() {
        return Err(invalid_input(
            CURRENT_BRANCH,
            "Git returned an empty HEAD reference",
        ));
    }

    Ok(GitBranchOutput {
        repo_root: root.to_string_lossy().into_owned(),
        branch,
        detached,
    })
}

fn status(invocation: &NativeInvocation<'_, GitInput>) -> Result<GitStatusOutput, RuntimeError> {
    let root = resolve_repo_root_for(
        STATUS,
        &invocation.inputs.repo_root,
        invocation.env,
        invocation.skill_directory,
        invocation.bounds,
    )?;
    let env = git_env(invocation)?;
    let outcome = run_git(&root, &env, &["status", "--short", "--branch"])?;
    if !outcome.status.success() {
        return Err(invalid_input(
            STATUS,
            "repo_root must be a readable Git working tree",
        ));
    }
    let output = output_text(STATUS, invocation, outcome)?;
    let mut lines = output.lines();
    let first = lines.next();
    let (branch, entries) = match first {
        Some(line) if line.starts_with("## ") => (
            Some(line.trim_start_matches("## ").to_owned()),
            lines.map(str::to_owned).collect::<Vec<_>>(),
        ),
        Some(line) if !line.is_empty() => (
            None,
            std::iter::once(line.to_owned())
                .chain(lines.map(str::to_owned))
                .collect(),
        ),
        _ => (None, Vec::new()),
    };
    Ok(GitStatusOutput {
        repo_root: root.to_string_lossy().into_owned(),
        clean: entries.is_empty(),
        entries,
        branch,
    })
}

fn diff_name_only(
    invocation: &NativeInvocation<'_, GitDiffInput>,
) -> Result<GitDiffOutput, RuntimeError> {
    let root = resolve_repo_root_for(
        DIFF_NAME_ONLY,
        &invocation.inputs.repo_root,
        invocation.env,
        invocation.skill_directory,
        invocation.bounds,
    )?;
    let base = &invocation.inputs.base;
    validate_base(base)?;
    let env = git_env(invocation)?;
    let commitish = format!("{base}^{{commit}}");
    let resolved = run_git(&root, &env, &["rev-parse", "--verify", &commitish])?;
    if !resolved.status.success() {
        return Err(invalid_input(
            DIFF_NAME_ONLY,
            "base must resolve to a readable Git commit",
        ));
    }
    let commit = output_text(DIFF_NAME_ONLY, invocation, resolved)?;
    let outcome = run_git(
        &root,
        &env,
        &[
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--name-only",
            "--relative",
            &commit,
            "--",
        ],
    )?;
    if !outcome.status.success() {
        return Err(invalid_input(DIFF_NAME_ONLY, "Git diff failed"));
    }
    let files = output_text(DIFF_NAME_ONLY, invocation, outcome)?
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect();
    Ok(GitDiffOutput {
        repo_root: root.to_string_lossy().into_owned(),
        base: base.to_owned(),
        files,
    })
}

fn validate_base(base: &str) -> Result<(), RuntimeError> {
    validate_revision(DIFF_NAME_ONLY, "base", base)
}

/// A revision a tool passes to Git: bounded, and never read as an option.
fn validate_revision(tool: &str, field: &str, value: &str) -> Result<(), RuntimeError> {
    if value.is_empty()
        || value.starts_with('-')
        || value.len() > 1024
        || value.contains('\0')
        || value.chars().any(char::is_whitespace)
    {
        return Err(invalid_input(
            tool,
            format!("{field} must be a bounded Git ref, range or commit id"),
        ));
    }
    Ok(())
}

fn git_env<I: ?Sized>(
    invocation: &NativeInvocation<'_, I>,
) -> Result<std::collections::BTreeMap<String, String>, RuntimeError> {
    let mut env = process_base_environment(invocation.env)?;
    env.insert("GIT_OPTIONAL_LOCKS".to_owned(), "0".to_owned());
    env.insert("GIT_PAGER".to_owned(), "cat".to_owned());
    Ok(env)
}

fn run_git(
    root: &std::path::Path,
    env: &std::collections::BTreeMap<String, String>,
    args: &[&str],
) -> Result<ProcessOutcome, RuntimeError> {
    run_git_limited(root, env, args, OUTPUT_LIMIT_BYTES)
}

/// One supervised Git process: no pager, no fsmonitor, pathspecs taken
/// literally, output bounded at `limit` bytes.
fn run_git_limited(
    root: &std::path::Path,
    env: &std::collections::BTreeMap<String, String>,
    args: &[&str],
    limit: usize,
) -> Result<ProcessOutcome, RuntimeError> {
    let mut bounded_args = vec![
        "--no-pager".to_owned(),
        "--literal-pathspecs".to_owned(),
        "-c".to_owned(),
        "core.fsmonitor=false".to_owned(),
    ];
    bounded_args.extend(args.iter().map(|value| (*value).to_owned()));
    run_process(
        ProcessSpec::new("native Git observation", "git", limit)
            .args(bounded_args)
            .cwd(root)
            .env(env.clone())
            .timeout(Some(TIMEOUT)),
    )
    .map_err(|error| invalid_input("git.observe", error.to_string()))
}

fn output_text<I: ?Sized>(
    tool: &str,
    invocation: &NativeInvocation<'_, I>,
    outcome: ProcessOutcome,
) -> Result<String, RuntimeError> {
    let (text, truncated) = output_bounded(tool, invocation, outcome, OUTPUT_LIMIT_BYTES)?;
    if truncated {
        return Err(invalid_input(
            tool,
            "Git observation exceeded runtime bounds",
        ));
    }
    Ok(text.trim().to_owned())
}

/// A Git process's redacted output and whether it was cut at `limit`; a
/// timeout or an overlong error stream is still a failure.
fn output_bounded<I: ?Sized>(
    tool: &str,
    invocation: &NativeInvocation<'_, I>,
    outcome: ProcessOutcome,
    limit: usize,
) -> Result<(String, bool), RuntimeError> {
    if outcome.timed_out || outcome.stderr.truncated {
        return Err(invalid_input(
            tool,
            "Git observation exceeded runtime bounds",
        ));
    }
    let truncated = outcome.stdout.truncated;
    Ok((
        invocation
            .credential_delivery
            .redact_bytes_to_string(outcome.stdout.bytes, limit),
        truncated,
    ))
}

#[cfg(test)]
mod tests;
