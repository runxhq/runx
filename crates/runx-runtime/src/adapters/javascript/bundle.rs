use std::collections::{BTreeMap, BTreeSet, VecDeque};

use runx_contracts::javascript_worker::InvocationLimits;

use super::PreparedJavaScriptInvocation;
use crate::{RuntimeError, SkillInvocation};

pub(super) fn validated_module(
    request: &SkillInvocation,
) -> Result<PreparedJavaScriptInvocation, RuntimeError> {
    let loaded = crate::load_validated_skill_package(&request.skill_directory)?;
    validated_module_from_package(request, &loaded)
}

pub(super) fn validated_module_from_package(
    request: &SkillInvocation,
    loaded: &crate::LoadedSkillPackage,
) -> Result<PreparedJavaScriptInvocation, RuntimeError> {
    let worker = super::worker(request)?;
    let entry_module = package_relative(
        profile_directory(loaded.profile_path.as_deref()),
        &worker.module,
    );
    let modules = reachable_modules(&loaded.package, &entry_module)?;
    let mut limits = InvocationLimits::default();
    if let Some(timeout_seconds) = worker.timeout_seconds {
        limits.wall_milliseconds =
            timeout_seconds
                .checked_mul(1_000)
                .ok_or_else(|| RuntimeError::JavaScriptWorker {
                    message: "JavaScript wall limit overflowed milliseconds".to_owned(),
                })?;
    }
    let limits = limits
        .validate()
        .map_err(|error| RuntimeError::JavaScriptWorker {
            message: format!("JavaScript invocation limits are invalid: {error}"),
        })?;
    Ok(PreparedJavaScriptInvocation {
        entry_module,
        export_name: worker
            .export
            .clone()
            .unwrap_or_else(|| "default".to_owned()),
        modules,
        environment: crate::execution_environment::resolve_environment(
            &worker.environment,
            &request.env,
            &request.skill_directory,
        )?,
        worker_path: request.env.get(super::WORKER_PATH_ENV).cloned(),
        limits,
    })
}

fn reachable_modules(
    package: &runx_parser::ValidatedSkillPackage,
    entry_module: &str,
) -> Result<BTreeMap<String, String>, RuntimeError> {
    if !package.javascript_modules.contains_key(entry_module) {
        return Err(RuntimeError::InvalidProcessInvocation {
            message: format!(
                "javascript entry module {entry_module:?} is not in the aggregate validated package bundle"
            ),
        });
    }
    let mut queue = VecDeque::from([entry_module.to_owned()]);
    let mut seen = BTreeSet::new();
    let mut modules = BTreeMap::new();
    while let Some(path) = queue.pop_front() {
        if !seen.insert(path.clone()) {
            continue;
        }
        let metadata = package.javascript_modules.get(&path).ok_or_else(|| {
            RuntimeError::InvalidProcessInvocation {
                message: format!("validated JavaScript dependency {path:?} is missing"),
            }
        })?;
        let source =
            package
                .file_text(&path)
                .ok_or_else(|| RuntimeError::InvalidProcessInvocation {
                    message: format!(
                        "validated JavaScript dependency {path:?} is not UTF-8 source"
                    ),
                })?;
        modules.insert(path.clone(), source.to_owned());
        for specifier in &metadata.imports {
            queue.push_back(
                runx_parser::resolve_javascript_module_import(&path, specifier)
                    .map_err(RuntimeError::from)?,
            );
        }
    }
    Ok(modules)
}

fn profile_directory(profile_path: Option<&str>) -> &str {
    profile_path
        .and_then(runx_parser::profile_directory)
        .unwrap_or("")
}

fn package_relative(directory: &str, path: &str) -> String {
    if directory.is_empty() {
        path.to_owned()
    } else {
        format!("{directory}/{path}")
    }
}
