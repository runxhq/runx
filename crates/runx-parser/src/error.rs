use thiserror::Error;

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

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ValidationError {
    #[error("{field} is required.")]
    MissingField { field: String },
    #[error("{message}")]
    InvalidField { field: String, message: String },
}
