//! Connector digests, computed only in Rust over `runx.stable-json.v1`.
//!
//! - The manifest digest covers the whole parsed manifest.
//! - The operation digest covers the fields that change what a grant
//!   authorizes or what a person approves. Descriptions and implementations
//!   are excluded, so a copy edit or a fixed implementation is not an
//!   authority change.
//! - The implementation digest covers the implementation that ran, with
//!   the MCP server folded in, and for a process the exact tool package that
//!   received the credential. It is sealed per call, never pinned.
//! - The tool package digest covers every file a connector's tool ships.
//! - The connector digest covers what a connector executes: its manifest and
//!   the package of each tool its process operations run. Closures bind it.
//! - The credential declaration digest covers where a secret may be sent.
//!
//! Every digest is written straight from borrowed fields into canonical
//! bytes; no intermediate value is built.

use std::collections::BTreeMap;

use serde::Serialize;
use thiserror::Error;

use crate::{JsonObject, to_canonical_bytes};

use super::{
    AmountRule, ApprovalFloor, ConnectorName, ConnectorManifest, ConnectorOperation,
    CredentialDeclaration, CredentialName, Digest, IdempotencyKeyFormat, Implementation, McpServer,
    NonEmptyString, OperationGuarantees, OperationName, OperationTarget, OperationVerb,
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
    connector: &'a ConnectorName,
    operation: &'a OperationName,
    verb: OperationVerb,
    scopes: &'a [NonEmptyString],
    credentials: &'a [CredentialName],
    target: &'a OperationTarget,
    approval: ApprovalFloor,
    idempotency_key_format: IdempotencyKeyFormat,
    guarantees: &'a OperationGuarantees,
    amount: AmountRule,
    input_schema_digest: Option<Digest>,
}

pub fn operation_digest(
    connector: &ConnectorName,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_package: Option<&'a Digest>,
}

/// The implementation that runs: an MCP operation with its connector's
/// server, a process with the tool package it runs.
pub fn implementation_digest(
    implementation: &Implementation,
    mcp_server: Option<&McpServer>,
    tool_package: Option<&Digest>,
) -> Result<Digest, ConnectorDigestError> {
    stable_digest(&ImplementationIdentity {
        implementation,
        mcp_server: match implementation {
            Implementation::Mcp(_) => mcp_server,
            _ => None,
        },
        tool_package: match implementation {
            Implementation::Process(_) => tool_package,
            _ => None,
        },
    })
}

#[derive(Serialize)]
struct ToolPackageFile<'a> {
    path: &'a str,
    sha256: String,
}

/// A connector tool package's digest: each file's package-relative POSIX
/// path beside the digest of its bytes, in path order.
pub fn tool_package_digest<'a>(
    files: impl IntoIterator<Item = (&'a str, &'a [u8])>,
) -> Result<Digest, ConnectorDigestError> {
    let mut files = files
        .into_iter()
        .map(|(path, bytes)| ToolPackageFile {
            path,
            sha256: crate::sha256_hex(bytes),
        })
        .collect::<Vec<_>>();
    files.sort_by(|left, right| left.path.cmp(right.path));
    stable_digest(&files)
}

#[derive(Serialize)]
struct ConnectorIdentity<'a> {
    manifest: &'a Digest,
    tools: BTreeMap<&'a str, &'a Digest>,
}

/// A connector's digest: its manifest digest beside the package digest of
/// each tool its process operations run, by tool name. A changed tool changes
/// the connector even when the manifest does not. Its README and mark are
/// presentation and stay out.
pub fn connector_digest<'a>(
    manifest: &'a Digest,
    tools: impl IntoIterator<Item = (&'a str, &'a Digest)>,
) -> Result<Digest, ConnectorDigestError> {
    let mut by_name = BTreeMap::new();
    for (name, digest) in tools {
        if by_name.insert(name, digest).is_some() {
            return Err(ConnectorDigestError(format!(
                "connector tool {name} is listed twice"
            )));
        }
    }
    stable_digest(&ConnectorIdentity {
        manifest,
        tools: by_name,
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
