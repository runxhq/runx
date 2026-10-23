//! Native bounded Git history: commits, a commit or a file at a revision, and
//! patches. Within agent bounds no patch or file shows a secret: changed paths
//! are listed first, filtered by the one secret-path rule, and only the paths
//! it allows are shown, the rest named as omitted.

use std::path::Path;

use runx_contracts::schema::{BoundedVec, NonEmptyString};
use runx_contracts::{GIT_LOG_MAX_COMMITS, GitCommit, GitLog, GitPatch, GitShow};

use super::capability::{GitLogInput, GitPatchInput, GitShowInput};
use super::{
    NativeInvocation, git_env, invalid_input, non_empty, output_bounded, resolve_repo_root_for,
    root_name, run_git_limited, validate_revision,
};
use crate::RuntimeError;

pub(super) const LOG: &str = "git.log";
pub(super) const SHOW: &str = "git.show";
pub(super) const PATCH: &str = "git.diff";
const HISTORY_LIMIT_BYTES: usize = 1024 * 1024;
const MAX_LOG_COUNT: u64 = GIT_LOG_MAX_COMMITS as u64;
const MAX_PATHS: usize = 200;
const RECORD: char = '\u{1e}';
const FIELD: char = '\u{1f}';

pub(super) fn log(invocation: &NativeInvocation<'_, GitLogInput>) -> Result<GitLog, RuntimeError> {
    let inputs = invocation.inputs;
    let root = repo_root(LOG, &inputs.repo_root, invocation)?;
    validate_revision(LOG, "revision", &inputs.revision)?;
    if inputs.max_count == 0 || inputs.max_count > MAX_LOG_COUNT {
        return Err(invalid_input(
            LOG,
            format!("max_count must be 1-{MAX_LOG_COUNT}"),
        ));
    }
    validate_paths(LOG, &inputs.paths)?;
    let max_count = format!("--max-count={}", inputs.max_count);
    let since = inputs
        .since
        .as_deref()
        .map(|since| {
            if since.is_empty() || since.len() > 64 || since.chars().any(char::is_control) {
                return Err(invalid_input(
                    LOG,
                    "since must be a short date such as 2026-10-01 or 2.weeks.ago",
                ));
            }
            Ok(format!("--since={since}"))
        })
        .transpose()?;
    let format = format!("--format={RECORD}%H{FIELD}%an <%ae>{FIELD}%aI{FIELD}%s");
    let mut args = vec![
        "log",
        "--no-color",
        "--no-ext-diff",
        "--name-only",
        &format,
        &max_count,
    ];
    if let Some(since) = &since {
        args.push(since);
    }
    args.push(&inputs.revision);
    args.push("--");
    args.extend(inputs.paths.iter().map(String::as_str));
    let (text, truncated) = run(LOG, &root, &args, invocation)?;
    let mut commits = text
        .split(RECORD)
        .filter(|record| !record.trim().is_empty())
        .filter_map(commit)
        .collect::<Vec<_>>();
    if truncated {
        // The last record may be cut mid-list; keep only whole ones.
        commits.pop();
    }
    Ok(GitLog {
        repo_root: root_name(LOG, &root)?,
        revision: non_empty(LOG, "revision", inputs.revision.clone())?,
        commits: BoundedVec::new(commits).ok_or_else(|| {
            invalid_input(
                LOG,
                format!("Git returned more than {MAX_LOG_COUNT} commits"),
            )
        })?,
        truncated,
    })
}

fn commit(record: &str) -> Option<GitCommit> {
    let mut lines = record.lines();
    let mut fields = lines.next()?.split(FIELD);
    Some(GitCommit {
        commit: NonEmptyString::new(fields.next()?)?,
        author: fields.next()?.to_owned(),
        date: fields.next()?.to_owned(),
        subject: fields.next().unwrap_or_default().to_owned(),
        files: lines
            .map(str::trim)
            .filter_map(NonEmptyString::new)
            .collect(),
    })
}

pub(super) fn show(
    invocation: &NativeInvocation<'_, GitShowInput>,
) -> Result<GitShow, RuntimeError> {
    let inputs = invocation.inputs;
    let root = repo_root(SHOW, &inputs.repo_root, invocation)?;
    validate_revision(SHOW, "revision", &inputs.revision)?;
    if let Some(path) = &inputs.path {
        validate_paths(SHOW, std::slice::from_ref(path))?;
        invocation
            .bounds
            .admit(Path::new(path))
            .map_err(|error| invalid_input(SHOW, error.to_string()))?;
        let object = format!("{}:{path}", inputs.revision);
        let (text, truncated) = run(SHOW, &root, &["show", "--no-color", &object], invocation)?;
        let path = non_empty(SHOW, "path", path.clone())?;
        return Ok(GitShow {
            repo_root: root_name(SHOW, &root)?,
            revision: non_empty(SHOW, "revision", inputs.revision.clone())?,
            path: Some(path.clone()),
            text,
            files: vec![path],
            omitted: Vec::new(),
            truncated,
        });
    }
    let (names, _) = run(
        SHOW,
        &root,
        &[
            "show",
            "--no-color",
            "--format=",
            "--name-only",
            &inputs.revision,
        ],
        invocation,
    )?;
    let (files, omitted) = split_allowed(invocation, &names);
    let (header, _) = run(
        SHOW,
        &root,
        &[
            "show",
            "--no-color",
            "--no-patch",
            "--format=fuller",
            &inputs.revision,
        ],
        invocation,
    )?;
    let (patch, truncated) = if files.is_empty() {
        (String::new(), false)
    } else {
        let mut args = vec![
            "show",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--format=",
            "--patch",
            &inputs.revision,
            "--",
        ];
        args.extend(files.iter().map(NonEmptyString::as_str).take(MAX_PATHS));
        run(SHOW, &root, &args, invocation)?
    };
    Ok(GitShow {
        repo_root: root_name(SHOW, &root)?,
        revision: non_empty(SHOW, "revision", inputs.revision.clone())?,
        path: None,
        text: format!("{header}{patch}"),
        files,
        omitted,
        truncated: truncated || names.lines().count() > MAX_PATHS,
    })
}

pub(super) fn diff(
    invocation: &NativeInvocation<'_, GitPatchInput>,
) -> Result<GitPatch, RuntimeError> {
    let inputs = invocation.inputs;
    let root = repo_root(PATCH, &inputs.repo_root, invocation)?;
    validate_revision(PATCH, "base", &inputs.base)?;
    if let Some(head) = &inputs.head {
        validate_revision(PATCH, "head", head)?;
    }
    validate_paths(PATCH, &inputs.paths)?;
    let mut range = vec![inputs.base.as_str()];
    if let Some(head) = &inputs.head {
        range.push(head);
    }
    let mut names_args = vec![
        "diff",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--relative",
        "--name-only",
    ];
    names_args.extend(&range);
    names_args.push("--");
    names_args.extend(inputs.paths.iter().map(String::as_str));
    let (names, _) = run(PATCH, &root, &names_args, invocation)?;
    let (files, omitted) = split_allowed(invocation, &names);
    let (patch, truncated) = if files.is_empty() {
        (String::new(), false)
    } else {
        let mut args = vec![
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--relative",
            if inputs.stat { "--stat" } else { "--patch" },
        ];
        args.extend(&range);
        args.push("--");
        args.extend(files.iter().map(NonEmptyString::as_str).take(MAX_PATHS));
        run(PATCH, &root, &args, invocation)?
    };
    Ok(GitPatch {
        repo_root: root_name(PATCH, &root)?,
        base: non_empty(PATCH, "base", inputs.base.clone())?,
        head: inputs
            .head
            .clone()
            .map(|head| non_empty(PATCH, "head", head))
            .transpose()?,
        patch,
        files,
        omitted,
        truncated: truncated || names.lines().count() > MAX_PATHS,
    })
}

fn repo_root<I>(
    tool: &str,
    requested: &str,
    invocation: &NativeInvocation<'_, I>,
) -> Result<std::path::PathBuf, RuntimeError> {
    resolve_repo_root_for(
        tool,
        requested,
        invocation.env,
        invocation.skill_directory,
        invocation.bounds,
    )
}

fn run<I>(
    tool: &str,
    root: &Path,
    args: &[&str],
    invocation: &NativeInvocation<'_, I>,
) -> Result<(String, bool), RuntimeError> {
    let env = git_env(invocation)?;
    let outcome = run_git_limited(root, &env, args, HISTORY_LIMIT_BYTES)?;
    if !outcome.status.success() {
        return Err(invalid_input(
            tool,
            "Git could not read that history; check the repository root and revision",
        ));
    }
    output_bounded(tool, invocation, outcome, HISTORY_LIMIT_BYTES)
}

/// The changed paths a patch may show, and those the run's bounds keep out.
fn split_allowed<I>(
    invocation: &NativeInvocation<'_, I>,
    names: &str,
) -> (Vec<NonEmptyString>, Vec<NonEmptyString>) {
    let (mut files, mut omitted) = (Vec::new(), Vec::new());
    for name in names.lines().map(str::trim).filter_map(NonEmptyString::new) {
        if invocation.bounds.admit(Path::new(name.as_str())).is_ok() {
            files.push(name);
        } else {
            omitted.push(name);
        }
    }
    (files, omitted)
}

fn validate_paths(tool: &str, paths: &[String]) -> Result<(), RuntimeError> {
    if paths.len() > MAX_PATHS {
        return Err(invalid_input(
            tool,
            format!("paths may name at most {MAX_PATHS} entries"),
        ));
    }
    for path in paths {
        let candidate = Path::new(path);
        if path.is_empty()
            || path.starts_with('-')
            || path.contains('\0')
            || candidate.is_absolute()
            || candidate
                .components()
                .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            return Err(invalid_input(
                tool,
                format!("{path:?} must be a relative path inside the repository"),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
