//! Connector digests, computed only in Rust over `runx.stable-json.v1`.
//!
//! - The manifest digest covers the whole parsed manifest.
//! - The operation digest covers the fields that change what a grant
//!   authorizes or what a person approves. Descriptions and implementations
//!   are excluded, so a copy edit or a fixed implementation is not an
//!   authority change.
//! - The implementation digest covers the implementation that ran,
//!   with the MCP server folded in. It is sealed per call, never pinned.
//! - The credential declaration digest covers where a secret may be sent.
//!
//! Every digest is written straight from borrowed fields into canonical
//! bytes; no intermediate value is built.

use serde::Serialize;
use thiserror::Error;

use crate::{JsonObject, to_canonical_bytes};

use super::{
    AmountRule, ApprovalFloor, ConnectorId, ConnectorManifest, ConnectorOperation,
    CredentialDeclaration, CredentialName, Digest, IdempotencyKeyFormat, Implementation, McpServer,
    NonEmptyString, OperationGuarantees, OperationName, OperationTarget, OperationVerb,
    StandingEligibility,
};

#[derive(Clone, Debug, Error, PartialEq, Eq)]
#[error("connector digest could not be computed: {0}")]
pub struct ConnectorDigestError(String);

/// The digest of any contract value under `runx.stable-json.v1`.
pub fn stable_digest<T: Serialize + ?Sized>(value: &T) -> Result<Digest, ConnectorDigestError> {
    to_canonical_bytes(value)
        .map(|bytes| Digest::of(&bytes))
        .map_err(|error| ConnectorDigestError(error.to_string()))
}

pub fn manifest_digest(manifest: &ConnectorManifest) -> Result<Digest, ConnectorDigestError> {
    stable_digest(manifest)
}

#[derive(Serialize)]
struct OperationAuthority<'a> {
    connector: &'a ConnectorId,
    operation: &'a OperationName,
    verb: OperationVerb,
    scopes: &'a [NonEmptyString],
    credentials: &'a [CredentialName],
    target: &'a OperationTarget,
    approval: ApprovalFloor,
    standing: Option<&'a StandingEligibility>,
    idempotency_key_format: IdempotencyKeyFormat,
    guarantees: &'a OperationGuarantees,
    amount: AmountRule,
    input_schema_digest: Option<Digest>,
}

pub fn operation_digest(
    connector: &ConnectorId,
    name: &OperationName,
    operation: &ConnectorOperation,
) -> Result<Digest, ConnectorDigestError> {
    stable_digest(&OperationAuthority {
        connector,
        operation: name,
        verb: operation.verb,
        scopes: &operation.scopes,
        credentials: &operation.credentials,
        target: &operation.target,
        approval: operation.approval,
        standing: operation.standing.as_ref(),
        idempotency_key_format: operation.idempotency_key_format,
        guarantees: &operation.guarantees,
        amount: operation.amount,
        input_schema_digest: input_schema_digest(operation.input_schema.as_ref())?,
    })
}

#[derive(Serialize)]
struct ImplementationIdentity<'a> {
    implementation: &'a Implementation,
    #[serde(skip_serializing_if = "Option::is_none")]
    mcp_server: Option<&'a McpServer>,
}

pub fn implementation_digest(
    implementation: &Implementation,
    mcp_server: Option<&McpServer>,
) -> Result<Digest, ConnectorDigestError> {
    stable_digest(&ImplementationIdentity {
        implementation,
        mcp_server: match implementation {
            Implementation::Mcp(_) => mcp_server,
            _ => None,
        },
    })
}

/// What a served MCP tool is pinned by: its name, description, input schema
/// and annotations, exactly as `tools/list` served them.
#[derive(Serialize)]
struct ServedMcpTool<'a> {
    name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<&'a str>,
    #[serde(rename = "inputSchema", skip_serializing_if = "Option::is_none")]
    input_schema: Option<&'a crate::JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    annotations: Option<&'a crate::JsonValue>,
}

/// The digest an `mcp` implementation pins, over one `tools/list` entry.
pub fn mcp_tool_digest(tool: &crate::JsonObject) -> Result<Digest, ConnectorDigestError> {
    let name = tool
        .get("name")
        .and_then(crate::JsonValue::as_str)
        .ok_or_else(|| ConnectorDigestError("a served MCP tool has no name".to_owned()))?;
    stable_digest(&ServedMcpTool {
        name,
        description: tool.get("description").and_then(crate::JsonValue::as_str),
        input_schema: tool.get("inputSchema"),
        annotations: tool.get("annotations"),
    })
}

pub fn credential_declaration_digest(
    declaration: &CredentialDeclaration,
) -> Result<Digest, ConnectorDigestError> {
    stable_digest(declaration)
}

pub fn input_schema_digest(
    schema: Option<&JsonObject>,
) -> Result<Option<Digest>, ConnectorDigestError> {
    schema.map(stable_digest).transpose()
}
