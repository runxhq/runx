use std::fmt;

use runx_contracts::JsonValue;

mod dispatch;
mod input;
mod limits;

use dispatch::evaluate_kernel_input;
use input::{KernelInput, is_supported_kernel_kind, kernel_document_kind};
use limits::{validate_kernel_document_shape, validate_kernel_source_size};

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum KernelEvalOutput {
    Output { value: JsonValue },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum KernelEvalError {
    InvalidDocument(String),
    InvalidInput(String),
    SerializeOutput(String),
}

impl KernelEvalError {
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidDocument(_) => "invalid_document",
            Self::InvalidInput(_) => "invalid_input",
            Self::SerializeOutput(_) => "serialize_output",
        }
    }
}

impl fmt::Display for KernelEvalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDocument(message)
            | Self::InvalidInput(message)
            | Self::SerializeOutput(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for KernelEvalError {}

pub fn evaluate_kernel_document_str(source: &str) -> Result<KernelEvalOutput, KernelEvalError> {
    validate_kernel_source_size(source)?;
    let document = serde_json::from_str::<JsonValue>(source)
        .map_err(|error| KernelEvalError::InvalidDocument(error.to_string()))?;
    validate_kernel_document_shape(&document)?;
    if let Some(kind) = kernel_document_kind(&document)
        && !is_supported_kernel_kind(kind)
    {
        return Err(KernelEvalError::InvalidInput(format!(
            "unsupported kernel input kind '{kind}'"
        )));
    }
    let input = serde_json::from_str::<KernelInput>(source)
        .map_err(|error| KernelEvalError::InvalidInput(error.to_string()))?;
    Ok(KernelEvalOutput::Output {
        value: evaluate_kernel_input(input)?,
    })
}

#[cfg(test)]
mod tests {
    use super::limits::{MAX_KERNEL_EVAL_DOCUMENT_BYTES, MAX_KERNEL_EVAL_JSON_DEPTH};
    use super::*;

    #[test]
    fn evaluates_supported_document_under_limits() -> Result<(), KernelEvalError> {
        let output = evaluate_kernel_document_str(
            r#"{"kind":"policy.scopeGrantAllows","grantedScope":"repo:read","requestedScope":"repo:read","policy":"exact_only"}"#,
        )?;

        assert_eq!(
            output,
            KernelEvalOutput::Output {
                value: JsonValue::Bool(true)
            }
        );
        Ok(())
    }

    #[test]
    fn rejects_oversized_kernel_eval_source_before_parse() {
        let source = " ".repeat(MAX_KERNEL_EVAL_DOCUMENT_BYTES + 1);
        let error = assert_invalid_input(&source);

        assert_eq!(error.code(), "invalid_input");
        assert!(error.to_string().contains("kernel eval input exceeds"));
    }

    #[test]
    fn rejects_deep_kernel_eval_json_before_dispatch() {
        let source = format!(
            "{}0{}",
            "[".repeat(MAX_KERNEL_EVAL_JSON_DEPTH),
            "]".repeat(MAX_KERNEL_EVAL_JSON_DEPTH),
        );
        let error = assert_invalid_input(&source);

        assert_eq!(error.code(), "invalid_input");
        assert!(error.to_string().contains("exceeds JSON depth"));
    }

    fn assert_invalid_input(source: &str) -> KernelEvalError {
        match evaluate_kernel_document_str(source) {
            Err(error @ KernelEvalError::InvalidInput(_)) => error,
            Err(error) => error,
            Ok(output) => {
                KernelEvalError::InvalidInput(format!("expected invalid_input, got {output:?}"))
            }
        }
    }
}
