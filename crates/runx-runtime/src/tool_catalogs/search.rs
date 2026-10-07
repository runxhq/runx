use runx_contracts::tools::{ToolBuildStatus, ToolCatalogSearchReport};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolSearchOptions {
    pub query: String,
    pub source: Option<String>,
    pub limit: usize,
}

pub fn search_tools(options: &ToolSearchOptions) -> ToolCatalogSearchReport {
    search_tools_with_effects(options, &crate::RuntimeEffectRegistry::default())
}

pub fn search_tools_with_effects(
    options: &ToolSearchOptions,
    effects: &crate::RuntimeEffectRegistry,
) -> ToolCatalogSearchReport {
    let source = options.source.as_deref().unwrap_or("all").to_owned();
    let normalized_source = options
        .source
        .as_deref()
        .map(|source| source.trim().to_ascii_lowercase());
    let mut results = Vec::new();

    if native_catalog_allowed(normalized_source.as_deref(), effects) {
        let mut native = super::native::search(&options.query, options.limit, effects);
        if let Some(source) = exact_native_source(normalized_source.as_deref()) {
            native.retain(|tool| tool.source.eq_ignore_ascii_case(source));
        }
        results.extend(native);
    }

    ToolCatalogSearchReport {
        status: ToolBuildStatus::Success,
        query: options.query.clone(),
        source,
        results,
    }
}

fn native_catalog_allowed(source: Option<&str>, effects: &crate::RuntimeEffectRegistry) -> bool {
    matches!(
        source,
        None | Some("") | Some("all") | Some("native") | Some("runx-runtime")
    ) || source.is_some_and(|source| {
        effects
            .capabilities()
            .iter()
            .any(|capability| capability.definition().owner.eq_ignore_ascii_case(source))
    })
}

fn exact_native_source(source: Option<&str>) -> Option<&str> {
    match source {
        None | Some("") | Some("all") | Some("native") => None,
        Some(source) => Some(source),
    }
}
