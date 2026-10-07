//! One native source registry shared by direct skill runs, graph steps, and
//! harness replay. Source support and credential evidence must not vary by
//! entry point.

use runx_parser::SourceKind;

use crate::RuntimeError;
use crate::adapter::{InvocationOutput, SkillAdapter, SkillInvocation};
#[cfg(feature = "cli-tool")]
use crate::adapters::cli_tool::CliToolAdapter;

#[derive(Clone, Debug, Default)]
pub(crate) struct SkillSourceAdapter {
    javascript: crate::adapters::javascript::JavaScriptAdapter,
    package: Option<std::sync::Arc<crate::LoadedSkillPackage>>,
}

impl SkillSourceAdapter {
    #[must_use]
    pub(crate) const fn with_javascript(
        javascript: crate::adapters::javascript::JavaScriptAdapter,
    ) -> Self {
        Self {
            javascript,
            package: None,
        }
    }
    pub(crate) fn with_package(
        mut self,
        package: Option<std::sync::Arc<crate::LoadedSkillPackage>>,
    ) -> Self {
        self.package = package;
        self
    }
}

impl SkillAdapter for SkillSourceAdapter {
    fn adapter_type(&self) -> &'static str {
        "skill-source"
    }

    fn invoke(&self, request: SkillInvocation) -> Result<InvocationOutput, RuntimeError> {
        let credential_observation = request.credential_delivery.public_observation().cloned();
        let source_type = request.source.source_type;
        let mut output = match source_type {
            #[cfg(feature = "cli-tool")]
            SourceKind::CliTool => CliToolAdapter.invoke(request),
            SourceKind::JavaScript => match &self.package {
                Some(package) => self.javascript.invoke_from_package(request, package),
                None => self.javascript.invoke(request),
            },
            unsupported => Err(RuntimeError::UnsupportedSource {
                source_kind: unsupported.as_str().to_owned(),
            }),
        }?;
        if let Some(observation) = &credential_observation {
            output.record_credential_observation(observation)?;
        }
        Ok(output)
    }
}
