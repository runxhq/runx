use std::collections::{BTreeMap, BTreeSet, VecDeque};

use runx_contracts::sha256_prefixed;

use super::super::javascript::module_imports;
use super::super::path::normalize_module_import;
use super::super::{SkillPackageError, SkillPackageSource, ValidatedJavaScriptModule};
use super::contract::{has_nested_manual_boundary, text_file};

pub(super) fn validate_modules(
    source: &SkillPackageSource,
    roots: BTreeSet<String>,
    execution_files: &BTreeSet<String>,
) -> Result<BTreeMap<String, ValidatedJavaScriptModule>, SkillPackageError> {
    let mut queue = VecDeque::from_iter(roots);
    let mut modules = BTreeMap::new();
    while let Some(path) = queue.pop_front() {
        if modules.contains_key(&path) {
            continue;
        }
        let module = validate_module(source, &path, &mut queue)?;
        modules.insert(path, module);
    }
    reject_unreferenced_modules(source, &modules, execution_files)?;
    Ok(modules)
}

fn validate_module(
    source: &SkillPackageSource,
    path: &str,
    queue: &mut VecDeque<String>,
) -> Result<ValidatedJavaScriptModule, SkillPackageError> {
    let contents = source
        .files
        .get(path)
        .ok_or_else(|| SkillPackageError::invalid(path, "declared JavaScript module is missing"))?;
    let contents = text_file(path, contents)?;
    let imports = module_imports(path, contents)?;
    for specifier in &imports {
        let resolved = normalize_module_import(path, specifier)?;
        if !source.files.contains_key(&resolved) {
            return Err(SkillPackageError::invalid(
                path,
                format!("JavaScript import {specifier:?} resolves to missing file {resolved}"),
            ));
        }
        queue.push_back(resolved);
    }
    Ok(ValidatedJavaScriptModule {
        path: path.to_owned(),
        digest: sha256_prefixed(contents.as_bytes()),
        imports,
    })
}

fn reject_unreferenced_modules(
    source: &SkillPackageSource,
    modules: &BTreeMap<String, ValidatedJavaScriptModule>,
    execution_files: &BTreeSet<String>,
) -> Result<(), SkillPackageError> {
    for path in source
        .files
        .keys()
        .filter(|path| is_package_module(path, source))
    {
        if !modules.contains_key(path) && !execution_files.contains(path) {
            return Err(SkillPackageError::invalid(
                path,
                "executable JavaScript module is not reachable from a declared package source",
            ));
        }
    }
    Ok(())
}

fn is_package_module(path: &str, source: &SkillPackageSource) -> bool {
    if (!path.ends_with(".js") && !path.ends_with(".mjs"))
        || path.ends_with(".test.js")
        || path.ends_with(".test.mjs")
    {
        return false;
    }
    if path
        .split('/')
        .any(|segment| matches!(segment, "fixtures" | "test" | "tests" | "tools"))
    {
        return false;
    }
    !has_nested_manual_boundary(path, source)
}
