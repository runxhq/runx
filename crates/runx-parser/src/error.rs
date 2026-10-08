use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParseErrorKind {
    InvalidYaml,
    InvalidJson,
    InvalidDocument,
    UnsupportedScalar,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ParseError {
    #[error("{field}: YAML parse failed: {message}")]
    InvalidYaml { field: String, message: String },
    #[error("{field}: JSON parse failed: {message}")]
    InvalidJson { field: String, message: String },
    #[error("{field}: {message}")]
    InvalidDocument { field: String, message: String },
    #[error("{field}: scalar form is outside the supported YAML subset: {literal}")]
    UnsupportedScalar { field: String, literal: String },
}

impl ParseError {
    #[must_use]
    pub const fn kind(&self) -> ParseErrorKind {
        match self {
            Self::InvalidYaml { .. } => ParseErrorKind::InvalidYaml,
            Self::InvalidJson { .. } => ParseErrorKind::InvalidJson,
            Self::InvalidDocument { .. } => ParseErrorKind::InvalidDocument,
            Self::UnsupportedScalar { .. } => ParseErrorKind::UnsupportedScalar,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationErrorKind {
    MissingField,
    InvalidField,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ValidationError {
    #[error("{field} is required.")]
    MissingField { field: String },
    #[error("{message}")]
    InvalidField { field: String, message: String },
}

impl ValidationError {
    #[must_use]
    pub const fn kind(&self) -> ValidationErrorKind {
        match self {
            Self::MissingField { .. } => ValidationErrorKind::MissingField,
            Self::InvalidField { .. } => ValidationErrorKind::InvalidField,
        }
    }
}
