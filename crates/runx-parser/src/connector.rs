//! Connector manifest parsing and validation.
//!
//! Serde enforces shape (unknown fields, identifier grammars, non-empty
//! lists). This module enforces the rules that span fields: auth and
//! placement agreement, read/write invariants, implementation limits and the
//! multiplexed MCP tool rule. Checks that need other files (a process tool's
//! manifest and plugin) run in package validation and at runtime load.

use std::collections::{BTreeMap, BTreeSet};

use runx_contracts::connector::{
    AmountRule, ApprovalFloor, ConnectorAuth, ConnectorManifest, ConnectorOperation,
    CredentialName, Digest, HostedAuth, HttpAuth, HttpImplementation, HttpMethod, HttpReadback,
    HttpRequest, Implementation, LocalAuth, McpServer, OperationAccess, OperationName,
    OperationVerb, ProviderIdempotency, ReadbackExpected, ReadbackGuarantee, ReferenceDeclaration,
    PrincipalRequest, SessionAuth, Template, manifest_digest, operation_digest,
};
use runx_contracts::schema::NonEmptyString;
use runx_contracts::{JsonObject, JsonValue, ReferenceType};

use crate::{ParseError, ValidationError};

const MAX_HTTP_HOSTS: usize = 4;
const MAX_HTTP_RESULT_PATHS: usize = 25;
const MAX_HTTP_READ_REQUESTS: usize = 50;
const MAX_HTTP_READBACKS: usize = 10;
/// A connector process tool's name: lowercase dotted segments, each a
/// directory below the connector's `tools/`.
const PROCESS_TOOL_PATTERN: &str = "[a-z][a-z0-9_-]*(?:\\.[a-z][a-z0-9_-]*)+";

/// A manifest that passed every rule, with its digests computed once.
#[derive(Clone, Debug, PartialEq)]
pub struct ValidatedConnector {
    pub manifest: ConnectorManifest,
    pub manifest_digest: Digest,
    pub operation_digests: BTreeMap<OperationName, Digest>,
}

pub fn parse_connector_manifest(yaml: &str) -> Result<ConnectorManifest, ParseError> {
    crate::yaml::parse_yaml_document(yaml)
}

/// Validate a manifest.
///
/// # Errors
///
/// Returns every violated rule, each naming the field it concerns.
pub fn validate_connector_manifest(
    manifest: ConnectorManifest,
) -> Result<ValidatedConnector, Vec<ValidationError>> {
    let manifest = with_default_result_paths(manifest);
    let mut errors = Errors::default();
    validate_category(&manifest, &mut errors);
    validate_auth_and_placements(&manifest, &mut errors);
    validate_parameters(&manifest, &mut errors);
    for (name, operation) in &manifest.operations {
        validate_operation(&manifest, name, operation, &mut errors);
    }
    validate_mcp_multiplexing(&manifest, &mut errors);
    validate_references(&manifest.references, &mut errors);
    validate_effects(&manifest, &mut errors);
    validate_enforced(&manifest, &mut errors);
    validate_invocation(&manifest, &mut errors);
    errors.finish()?;

    let digest_error = |error: runx_contracts::connector::ConnectorDigestError| {
        vec![ValidationError::InvalidField {
            field: "manifest".to_owned(),
            message: error.to_string(),
        }]
    };
    let operation_digests = manifest
        .operations
        .iter()
        .map(|(name, operation)| {
            operation_digest(&manifest.id, name, operation).map(|digest| (name.clone(), digest))
        })
        .collect::<Result<BTreeMap<_, _>, _>>()
        .map_err(digest_error)?;
    Ok(ValidatedConnector {
        manifest_digest: manifest_digest(&manifest).map_err(digest_error)?,
        operation_digests,
        manifest,
    })
}

/// An HTTP operation with one result field takes the whole decoded answer
/// unless it says where the field comes from. The default is written into
/// the manifest, so every reader of it sees the same declaration.
fn with_default_result_paths(mut manifest: ConnectorManifest) -> ConnectorManifest {
    for operation in manifest.operations.values_mut() {
        if let (Some(Implementation::Http(http)), [field]) = (
            operation.implementation.as_mut(),
            operation.result_fields.as_slice(),
        ) && http.result_paths.is_empty()
        {
            http.result_paths.insert(
                field.as_str().to_owned(),
                NonEmptyString::from("acknowledgement"),
            );
        }
    }
    manifest
}

#[derive(Default)]
struct Errors(Vec<ValidationError>);

impl Errors {
    fn push(&mut self, field: &str, message: impl Into<String>) {
        self.0.push(ValidationError::InvalidField {
            field: field.to_owned(),
            message: message.into(),
        });
    }

    fn finish(self) -> Result<(), Vec<ValidationError>> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(self.0)
        }
    }
}

/// A declared category must be a connector category of the registry taxonomy.
fn validate_category(manifest: &ConnectorManifest, errors: &mut Errors) {
    if let Some(category) = &manifest.category
        && let Err(reason) = runx_contracts::validate_registry_category(
            runx_contracts::TaxonomySubject::Connector,
            category.as_str(),
        )
    {
        errors.push("category", reason);
    }
}

fn validate_auth_and_placements(manifest: &ConnectorManifest, errors: &mut Errors) {
    let auth = &manifest.auth;
    if auth.delegated.is_none()
        && auth.credentials.is_empty()
        && auth.session.is_none()
        && auth.none.is_none()
    {
        errors.push("auth", "declare at least one auth method");
    }
    if manifest.placements.hosted.is_none() && manifest.placements.local.is_none() {
        errors.push("placements", "declare at least one placement");
    }
    for (name, declaration) in &auth.credentials {
        match (store_only(manifest, name), declaration.audience.is_empty()) {
            (true, false) => errors.push(
                &format!("auth.credentials.{name}.audience"),
                "a credential only a store uses goes to the store its instance pins; declare no audience",
            ),
            (false, true) => errors.push(
                &format!("auth.credentials.{name}.audience"),
                "declare the hosts the credential may be sent to",
            ),
            _ => {}
        }
        for field in declaration.fields.iter() {
            let distinctive = |prefix: &NonEmptyString| {
                prefix.as_str().len() >= 4
                    && prefix
                        .as_str()
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
            };
            if !field.prefixes.is_empty()
                && (!field.secret || !field.prefixes.iter().all(distinctive))
            {
                errors.push(
                    &format!("auth.credentials.{name}.fields"),
                    format!(
                        "field {} declares prefixes only as a secret, each at least four letters, digits, _ or -",
                        field.name
                    ),
                );
            }
        }
    }
    if let Some(hosted) = &manifest.placements.hosted {
        match hosted.auth {
            HostedAuth::Delegated => {
                if auth.delegated.is_none() {
                    errors.push(
                        "placements.hosted.auth",
                        "delegated is not declared under auth",
                    );
                }
                if hosted.credential.is_some() {
                    errors.push(
                        "placements.hosted.credential",
                        "delegated placement does not name a stored credential",
                    );
                }
            }
            HostedAuth::Credential => match &hosted.credential {
                Some(name) if auth.credentials.contains_key(name) => {}
                Some(name) => errors.push(
                    "placements.hosted.credential",
                    format!("credential {name} is not declared under auth.credentials"),
                ),
                None => errors.push(
                    "placements.hosted.credential",
                    "credential placement must name the stored credential",
                ),
            },
        }
    }
    if let Some(local) = &manifest.placements.local {
        for (index, method) in local.auth.iter().enumerate() {
            if local.auth[..index].contains(method) {
                errors.push("placements.local.auth", "auth methods must be unique");
            }
            let declared = match method {
                LocalAuth::Credentials => !auth.credentials.is_empty(),
                LocalAuth::Session => auth.session.is_some(),
                LocalAuth::None => auth.none.is_some(),
            };
            if !declared {
                errors.push(
                    "placements.local.auth",
                    format!("{} is not declared under auth", local_auth_name(*method)),
                );
            }
        }
        let uses_mcp = manifest
            .operations
            .values()
            .any(|operation| matches!(operation.implementation, Some(Implementation::Mcp(_))));
        if uses_mcp && local.mcp_server.is_none() {
            errors.push(
                "placements.local.mcp_server",
                "operations with mcp implementations need the connector's mcp_server",
            );
        }
        if !uses_mcp && local.mcp_server.is_some() {
            errors.push(
                "placements.local.mcp_server",
                "mcp_server is declared but no operation uses it",
            );
        }
        if let Some(McpServer::StreamableHttp {
            url,
            auth: server_auth,
            headers,
        }) = &local.mcp_server
        {
            // The session's own protocol headers are the runtime's; a
            // manifest adds only headers the server reads.
            for (name, template) in headers {
                let field = format!("placements.local.mcp_server.headers.{name}");
                if name.is_empty()
                    || !name.bytes().all(|byte| {
                        byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'
                    })
                    || ["accept", "content-type"].contains(&name.as_str())
                    || mcp_protocol_header(name)
                {
                    errors.push(
                        &field,
                        "a header name is a lowercase token the runtime does not set itself",
                    );
                }
                if template.binds_exchange() {
                    errors.push(
                        &field,
                        "a session header binds only the call's arguments or instance parameters",
                    );
                }
            }
            let host = url::Url::parse(url.as_str())
                .ok()
                .filter(|url| url.scheme() == "https")
                .and_then(|url| url.host_str().map(str::to_ascii_lowercase));
            match host {
                None => errors.push("placements.local.mcp_server.url", "expected an https URL"),
                Some(host) => validate_auth(
                    auth,
                    server_auth,
                    "placements.local.mcp_server.auth",
                    &[host.as_str()],
                    errors,
                ),
            }
            validate_template_headers(
                server_auth,
                headers,
                "placements.local.mcp_server.headers",
                errors,
            );
            for (credential, presentation) in server_auth {
                if matches!(
                    presentation,
                    HttpAuth::Oauth1 | HttpAuth::Url { .. } | HttpAuth::Body { .. }
                ) {
                    errors.push(
                        &format!("placements.local.mcp_server.auth.{credential}"),
                        "an MCP server authenticates with a bearer token or a declared credential header",
                    );
                }
            }
            // Every credential an MCP operation accepts must say how it
            // authenticates the session; one it does not would never reach
            // the server.
            for (name, operation) in &manifest.operations {
                if !matches!(operation.implementation, Some(Implementation::Mcp(_))) {
                    continue;
                }
                for credential in &operation.credentials {
                    if !server_auth.contains_key(credential) {
                        errors.push(
                            &format!("operations.{name}.credentials"),
                            format!(
                                "the MCP server does not say how credential {credential} authenticates"
                            ),
                        );
                    }
                }
            }
        }
        if let Some(McpServer::Stdio { command, args }) = &local.mcp_server {
            const MAX_ARGS: usize = 32;
            const MAX_ARG_BYTES: usize = 1024;
            if command.as_str().chars().any(char::is_whitespace) {
                errors.push(
                    "placements.local.mcp_server.command",
                    "a command is one program; put its arguments in args",
                );
            }
            if args.len() > MAX_ARGS
                || args
                    .iter()
                    .any(|arg| arg.as_str().len() > MAX_ARG_BYTES || arg.as_str().contains('\0'))
            {
                errors.push(
                    "placements.local.mcp_server.args",
                    format!("at most {MAX_ARGS} arguments of at most {MAX_ARG_BYTES} bytes each"),
                );
            }
        }
    }
    if auth.delegated.is_some()
        && manifest
            .placements
            .hosted
            .as_ref()
            .is_none_or(|hosted| hosted.auth != HostedAuth::Delegated)
    {
        errors.push(
            "auth.delegated",
            "delegated auth needs a delegated hosted placement",
        );
    }
    if let Some(delegated) = &auth.delegated {
        validate_delegated_principal(manifest, &delegated.principal, errors);
    }
    if auth.session.is_some()
        && manifest
            .placements
            .local
            .as_ref()
            .is_none_or(|local| !local.auth.contains(&LocalAuth::Session))
    {
        errors.push(
            "auth.session",
            "session auth needs a local placement that accepts it",
        );
    }
    if let Some(SessionAuth::Command {
        command,
        credential,
        principal,
    }) = &auth.session
    {
        validate_command_session(auth, command, credential, principal, errors);
    }
}

/// Whether only `store` implementations use `credential`: it is then sent
/// to the store its instance pins and nowhere else.
fn store_only(manifest: &ConnectorManifest, credential: &CredentialName) -> bool {
    let auth = &manifest.auth;
    let session = matches!(
        &auth.session,
        Some(SessionAuth::Command { credential: stands_in, .. }) if stands_in == credential
    );
    let hosted = manifest
        .placements
        .hosted
        .as_ref()
        .is_some_and(|hosted| hosted.credential.as_ref() == Some(credential));
    let mut users = manifest
        .operations
        .values()
        .filter(|operation| operation.credentials.contains(credential))
        .peekable();
    users.peek().is_some()
        && !session
        && !hosted
        && users.all(|operation| {
            operation.hosted.is_none()
                && matches!(operation.implementation, Some(Implementation::Store(_)))
        })
}

/// The instance parameters a connector declares are a JSON Schema for an
/// object, and only a connector that declares them binds `$parameter`.
fn validate_parameters(manifest: &ConnectorManifest, errors: &mut Errors) {
    match &manifest.parameters {
        Some(schema) => {
            let valid = serde_json::to_value(schema)
                .map_err(|error| error.to_string())
                .and_then(|schema| {
                    jsonschema::draft202012::meta::validate(&schema)
                        .map_err(|error| error.to_string())
                });
            if let Err(reason) = valid {
                errors.push("parameters", format!("not a valid JSON Schema: {reason}"));
            }
            if schema.get("type").and_then(JsonValue::as_str) != Some("object") {
                errors.push("parameters", "instance parameters are an object");
            }
        }
        None => {
            for (name, operation) in &manifest.operations {
                let binds = operation.target.template.binds_parameter()
                    || matches!(&operation.implementation, Some(Implementation::Http(http))
                        if http.requests.iter().chain(
                            http.preconditions.iter().chain(&http.readbacks).map(|check| &check.request),
                        ).any(binds_parameter));
                if binds {
                    errors.push(
                        &format!("operations.{name}"),
                        "binds an instance parameter the connector does not declare",
                    );
                }
            }
        }
    }
}

/// A command session runs one program from `PATH`, stands in for a declared
/// one-field credential, and reads its principal inside that credential's
/// audience.
fn validate_command_session(
    auth: &ConnectorAuth,
    command: &[NonEmptyString],
    credential: &CredentialName,
    principal: &PrincipalRequest,
    errors: &mut Errors,
) {
    if command
        .first()
        .is_some_and(|program| program.as_str().contains(['/', '\\']))
    {
        errors.push(
            "auth.session.command",
            "the program is a name found on PATH, not a path",
        );
    }
    let Some(declaration) = auth.credentials.get(credential) else {
        errors.push(
            "auth.session.credential",
            format!("credential {credential} is not declared"),
        );
        return;
    };
    if declaration.fields.len() != 1 {
        errors.push(
            "auth.session.credential",
            format!(
                "a session token stands in for a one-field credential, and {credential} has several"
            ),
        );
    }
    match https_host(principal) {
        Some(host)
            if declaration
                .audience
                .iter()
                .any(|allowed| allowed.as_str() == host) => {}
        Some(host) => errors.push(
            "auth.session.principal.url",
            format!("host {host} is outside credential {credential}'s audience"),
        ),
        None => errors.push(
            "auth.session.principal.url",
            "the principal URL must be https",
        ),
    }
}

/// A delegated grant's principal request goes where its hosted operations
/// go: over https, to a host one of them declares. Operations a hosted driver
/// implements declare no hosts here; Connect holds every request under a
/// delegated grant, this one included, to its provider's host allowlist.
fn validate_delegated_principal(
    manifest: &ConnectorManifest,
    principal: &PrincipalRequest,
    errors: &mut Errors,
) {
    let declared: BTreeSet<&str> = manifest
        .operations
        .values()
        .filter(|operation| operation.hosted.is_some())
        .filter_map(|operation| match &operation.implementation {
            Some(Implementation::Http(http)) => Some(http),
            _ => None,
        })
        .flat_map(|http| http.allowed_hosts.iter().map(NonEmptyString::as_str))
        .collect();
    match https_host(principal) {
        Some(host) if declared.is_empty() || declared.contains(host.as_str()) => {}
        Some(host) => errors.push(
            "auth.delegated.principal.url",
            format!("host {host} is not one the connector's hosted operations reach"),
        ),
        None => errors.push(
            "auth.delegated.principal.url",
            "the principal URL must be https",
        ),
    }
}

fn https_host(principal: &PrincipalRequest) -> Option<String> {
    url::Url::parse(principal.url.as_str())
        .ok()
        .filter(|url| url.scheme() == "https")
        .and_then(|url| url.host_str().map(str::to_ascii_lowercase))
}

fn validate_operation(
    manifest: &ConnectorManifest,
    name: &OperationName,
    operation: &ConnectorOperation,
    errors: &mut Errors,
) {
    let field = format!("operations.{name}");
    let read = operation.access() == OperationAccess::Read;
    let auth = &manifest.auth;

    let mut seen_scopes = BTreeSet::new();
    for scope in operation.scopes.iter() {
        if !seen_scopes.insert(scope.as_str()) {
            errors.push(
                &format!("{field}.scopes"),
                format!("duplicate scope {scope}"),
            );
        }
    }
    if let Some(pattern) = &operation.target.pattern
        && let Err(error) = whole_match(pattern.as_str())
    {
        errors.push(
            &format!("{field}.target.pattern"),
            format!("not a valid regular expression: {error}"),
        );
    }
    if let Some(schema) = &operation.input_schema {
        let valid = serde_json::to_value(schema)
            .map_err(|error| error.to_string())
            .and_then(|schema| {
                jsonschema::draft202012::meta::validate(&schema).map_err(|error| error.to_string())
            });
        if let Err(reason) = valid {
            errors.push(
                &format!("{field}.input_schema"),
                format!("not a valid JSON Schema: {reason}"),
            );
        }
    }

    if read {
        if operation.approval == ApprovalFloor::Required {
            errors.push(
                &format!("{field}.approval"),
                "a read cannot require approval",
            );
        }
        if operation.amount == AmountRule::Required {
            errors.push(&format!("{field}.amount"), "a read carries no amount");
        }
    }

    for credential in &operation.credentials {
        if !auth.credentials.contains_key(credential) {
            errors.push(
                &format!("{field}.credentials"),
                format!("credential {credential} is not declared under auth.credentials"),
            );
        }
    }
    let local_uses_credentials = manifest
        .placements
        .local
        .as_ref()
        .is_some_and(|local| local.auth.contains(&LocalAuth::Credentials));
    if operation.implementation.is_some()
        && local_uses_credentials
        && operation.credentials.is_empty()
    {
        errors.push(
            &format!("{field}.credentials"),
            "name the credentials that may authorize this operation",
        );
    }
    // An approval alone never authorizes a write: a write to a provider takes
    // a credential, a session or a grant. A webhook whose URL is the
    // capability declares that URL as a credential presented as the URL.
    if !read
        && manifest.auth.unauthenticated()
        && !operation
            .implementation
            .as_ref()
            .is_some_and(Implementation::writes_runx_storage)
    {
        errors.push(
            &format!("{field}.verb"),
            "a write to a provider needs a credential, a session or a grant",
        );
    }
    let acknowledged = operation.guarantees.readback == ReadbackGuarantee::Unavailable;
    if acknowledged && read {
        errors.push(
            &format!("{field}.guarantees.readback"),
            "only a write can be unavailable for readback",
        );
    }

    match (&operation.hosted, &manifest.placements.hosted) {
        (Some(_), None) => errors.push(
            &format!("{field}.hosted"),
            "the connector has no hosted placement",
        ),
        (Some(hosted), Some(placement)) => match placement.auth {
            HostedAuth::Delegated if hosted.provider_scopes.is_empty() => errors.push(
                &format!("{field}.hosted.provider_scopes"),
                "delegated placement needs the provider's scopes",
            ),
            HostedAuth::Credential if !hosted.provider_scopes.is_empty() => errors.push(
                &format!("{field}.hosted.provider_scopes"),
                "credential placement has no provider scopes",
            ),
            _ => {}
        },
        (None, _) => {}
    }

    let Some(implementation) = &operation.implementation else {
        return;
    };
    if let (Some(_), Some(placement)) = (&operation.hosted, &manifest.placements.hosted)
        && let Implementation::Http(http) = implementation
    {
        let supported = match placement.auth {
            HostedAuth::Delegated => {
                !http.auth.is_empty()
                    && http
                        .auth
                        .values()
                        .all(|auth| matches!(auth, HttpAuth::Bearer { .. }))
            }
            HostedAuth::Credential => placement
                .credential
                .as_ref()
                .and_then(|name| http.auth.get(name))
                .is_some_and(|auth| {
                    !matches!(
                        auth,
                        HttpAuth::Oauth1 | HttpAuth::Url { .. } | HttpAuth::Body { .. }
                    )
                }),
        };
        if !supported {
            errors.push(
                &format!("{field}.implementation.http.auth"),
                "hosted delegated requests use bearer auth; stored credentials use bearer or declared header auth",
            );
        }
        if http
            .requests
            .iter()
            .any(|request| !request.responses().is_empty())
        {
            errors.push(
                &format!("{field}.implementation.http.requests"),
                "runx Connect renders a hosted operation's requests from the call alone",
            );
        }
        if http.requests.iter().any(|request| request.pages.is_some()) {
            errors.push(
                &format!("{field}.implementation.http.requests"),
                "runx Connect sends one request per declaration; a hosted read is not paged",
            );
        }
        if http.rate_limit.is_some() {
            errors.push(
                &format!("{field}.implementation.http.rate_limit"),
                "runx Connect answers with the status and body only; a hosted request cannot read a reset header",
            );
        }
        let requests = http.requests.iter().chain(
            http.preconditions
                .iter()
                .chain(&http.readbacks)
                .map(|check| &check.request),
        );
        if requests.into_iter().any(binds_parameter) || operation.target.template.binds_parameter()
        {
            errors.push(
                &format!("{field}.implementation.http"),
                "runx Connect renders hosted requests without instance parameters; bind none",
            );
        }
    }
    let hosted_http =
        operation.hosted.is_some() && matches!(implementation, Implementation::Http(_));
    if manifest.placements.local.is_none() && !hosted_http {
        errors.push(
            &format!("{field}.implementation"),
            "no placement runs this implementation",
        );
        return;
    }
    match implementation {
        Implementation::Http(http) => {
            validate_http(manifest, &field, operation, http, errors);
        }
        Implementation::Mcp(mcp) => {
            if operation.input_schema.is_none() {
                errors.push(
                    &format!("{field}.input_schema"),
                    "mcp operations declare the input schema they accept",
                );
            }
            match (operation.access(), &mcp.readback, acknowledged) {
                (OperationAccess::Read, Some(_), _) => {
                    errors.push(
                        &format!("{field}.implementation.mcp.readback"),
                        "a read has no readback",
                    );
                }
                (OperationAccess::Mutate, None, false) => errors.push(
                    &format!("{field}.implementation.mcp.readback"),
                    "a write is confirmed by a readback tool call",
                ),
                (OperationAccess::Mutate, Some(_), true) => errors.push(
                    &format!("{field}.implementation.mcp.readback"),
                    "a write the provider cannot read back declares no readback",
                ),
                _ => {}
            }
        }
        Implementation::Process(process) => {
            // The tool ships beside the connector, at tools/<segment>/...
            // named by the reference, so it is a dotted name and never a path.
            if !whole_match(PROCESS_TOOL_PATTERN).is_ok_and(|tool| tool.is_match(&process.tool)) {
                errors.push(
                    &format!("{field}.implementation.process.tool"),
                    "a process tool is a dotted name of lowercase segments, such as git.push",
                );
            }
            match (operation.access(), &process.readback, acknowledged) {
                (OperationAccess::Read, Some(_), _) => errors.push(
                    &format!("{field}.implementation.process.readback"),
                    "a read has no readback",
                ),
                (OperationAccess::Mutate, None, false) => errors.push(
                    &format!("{field}.implementation.process.readback"),
                    "a write is confirmed by the tool's readback",
                ),
                (OperationAccess::Mutate, Some(_), true) => errors.push(
                    &format!("{field}.implementation.process.readback"),
                    "a write the provider cannot read back declares no readback",
                ),
                _ => {}
            }
            let mut seen = BTreeSet::new();
            for scope in &process.runtime_scopes {
                if !seen.insert(scope.as_str()) {
                    errors.push(
                        &format!("{field}.implementation.process.runtime_scopes"),
                        format!("duplicate runtime scope {scope}"),
                    );
                }
            }
        }
        Implementation::Store(_) => {
            if operation.input_schema.is_none() {
                errors.push(
                    &format!("{field}.input_schema"),
                    "store operations declare the input schema they accept",
                );
            }
            if acknowledged {
                errors.push(
                    &format!("{field}.guarantees.readback"),
                    "a store write is confirmed by reading it back",
                );
            }
            if operation.hosted.is_some() {
                errors.push(&format!("{field}.hosted"), "a store runs only locally");
            }
        }
        Implementation::Artifact(_) => {
            if operation.input_schema.is_none() {
                errors.push(
                    &format!("{field}.input_schema"),
                    "artifact operations declare the input schema they accept",
                );
            }
            if acknowledged {
                errors.push(
                    &format!("{field}.guarantees.readback"),
                    "an artifact write is confirmed by reading it back",
                );
            }
            if operation.hosted.is_some() {
                errors.push(&format!("{field}.hosted"), "an artifact runs only locally");
            }
            if !operation.credentials.is_empty() {
                errors.push(
                    &format!("{field}.credentials"),
                    "an artifact implementation acts as the signed-in runx account, never a credential",
                );
            }
        }
        Implementation::Web(_) => {
            if operation.verb != OperationVerb::Read {
                errors.push(&format!("{field}.verb"), "a web implementation only reads");
            }
            if !operation.credentials.is_empty() {
                errors.push(
                    &format!("{field}.credentials"),
                    "a web implementation sends no credential",
                );
            }
        }
    }
}

/// A declared pattern as runx applies it: matched against the whole value.
pub fn whole_match(pattern: &str) -> Result<regex::Regex, regex::Error> {
    regex::Regex::new(&format!("^(?:{pattern})$"))
}

fn validate_references(references: &[ReferenceDeclaration], errors: &mut Errors) {
    for (index, reference) in references.iter().enumerate() {
        if let Err(error) = whole_match(reference.locator.as_str()) {
            errors.push(
                &format!("references[{index}].locator"),
                format!("not a valid regular expression: {error}"),
            );
        }
        // runx Connect compiles these too, as JavaScript expressions: keep to
        // the syntax both engines read alike, with no inline flags or named
        // groups.
        if reference.locator.as_str().replace("(?:", "").contains("(?") {
            errors.push(
                &format!("references[{index}].locator"),
                "use only (?: groups; inline flags and named groups read differently in runx Connect",
            );
        }
    }
}

/// An effect is what a write produced: a kind the connector's own references
/// declare, when it declares any, rendered from the call and its
/// result. `$result` is read by an effect and by nothing the call renders
/// before it has one.
fn validate_effects(manifest: &ConnectorManifest, errors: &mut Errors) {
    let declared = manifest
        .references
        .iter()
        .map(|reference| &reference.kind)
        .collect::<Vec<_>>();
    for (name, operation) in &manifest.operations {
        let field = format!("operations.{name}");
        if call_templates(manifest, operation).any(Template::binds_result) {
            errors.push(
                &field,
                "only an effect reads $result; the call has no result while it is made",
            );
        }
        let Some(effect) = &operation.effect else {
            continue;
        };
        let field = format!("{field}.effect");
        if operation.access() == OperationAccess::Read {
            errors.push(
                &field,
                "a read produces nothing; only a write declares an effect",
            );
        }
        if !declared.is_empty() && !declared.contains(&&effect.kind) {
            errors.push(
                &format!("{field}.kind"),
                format!(
                    "the connector's references declare no {} locator to check it against",
                    ReferenceType::from(effect.kind).as_str()
                ),
            );
        }
        let locator = &effect.locator;
        if locator.binds_exchange() && !(effect.each.is_some() && !binds_response(locator)) {
            errors.push(
                &format!("{field}.locator"),
                "a locator reads $item only under each, and never $response",
            );
        }
        if locator.is_literal() {
            errors.push(
                &format!("{field}.locator"),
                "a locator is rendered from the call's arguments and result",
            );
        }
        if let Some(each) = &effect.each
            && each.result_pointer().is_none()
        {
            errors.push(
                &format!("{field}.each"),
                "each names the result array the effect repeats over, as one $result binding",
            );
        }
    }
}

/// `enforced` means the provider deduplicates by the key the call carries,
/// which is what makes resending an uncertain write safe. A declared request
/// must therefore send that key, and a write repeated per item cannot carry
/// one key for every item.
fn validate_enforced(manifest: &ConnectorManifest, errors: &mut Errors) {
    for (name, operation) in &manifest.operations {
        let enforced = operation.guarantees.provider_idempotency == ProviderIdempotency::Enforced;
        let field = format!("operations.{name}.guarantees.provider_idempotency");
        // A read changes nothing for its provider to deduplicate, and the
        // kernel derives no key for it to send.
        if operation.access() == OperationAccess::Read {
            if enforced {
                errors.push(
                    &field,
                    "a read changes nothing for its provider to deduplicate",
                );
            }
            if call_templates(manifest, operation).any(Template::binds_idempotency_key) {
                errors.push(
                    &format!("operations.{name}"),
                    "a read has no idempotency key: it binds no {\"$runx\": \"idempotency_key\"}",
                );
            }
            continue;
        }
        if !enforced {
            continue;
        }
        let sends_key = match &operation.implementation {
            Some(Implementation::Http(http)) => {
                if http.requests.iter().any(|request| request.each.is_some()) {
                    errors.push(
                        &field,
                        "a write repeated per item sends one call key for every item, so its provider cannot enforce it",
                    );
                }
                http.requests
                    .iter()
                    .flat_map(HttpRequest::templates)
                    .any(Template::binds_idempotency_key)
            }
            Some(Implementation::Mcp(mcp)) => mcp.arguments.binds_idempotency_key(),
            _ => true,
        };
        if !sends_key {
            errors.push(
                &field,
                "enforced deduplication needs the request to send the call's key: bind {\"$runx\": \"idempotency_key\"}",
            );
        }
    }
}

fn binds_response(template: &Template) -> bool {
    !template.responses().is_empty()
}

/// Every template the call renders while it is made: its target, and its
/// implementation's requests, readbacks, tool arguments and session headers.
fn call_templates<'a>(
    manifest: &'a ConnectorManifest,
    operation: &'a ConnectorOperation,
) -> impl Iterator<Item = &'a Template> {
    std::iter::once(&operation.target.template).chain(implementation_templates(manifest, operation))
}

/// The implementation's requests, readbacks, tool arguments and session
/// headers.
fn implementation_templates<'a>(
    manifest: &'a ConnectorManifest,
    operation: &'a ConnectorOperation,
) -> impl Iterator<Item = &'a Template> {
    let mut templates = Vec::new();
    match &operation.implementation {
        Some(Implementation::Http(http)) => {
            templates.extend(http.requests.iter().flat_map(HttpRequest::templates));
            templates.extend(
                http.preconditions
                    .iter()
                    .chain(&http.readbacks)
                    .flat_map(|readback| readback.request.templates()),
            );
        }
        Some(Implementation::Mcp(mcp)) => {
            templates.push(&mcp.arguments);
            templates.extend(mcp.readback.iter().map(|readback| &readback.arguments));
            if let Some(McpServer::StreamableHttp { headers, .. }) = manifest
                .placements
                .local
                .as_ref()
                .and_then(|local| local.mcp_server.as_ref())
            {
                templates.extend(headers.values());
            }
        }
        _ => {}
    }
    templates.into_iter()
}

/// The paid invocation a call runs under is known only to runx Connect,
/// which renders a hosted operation's declared requests. The kernel renders
/// everything else and never holds it, so a binding there could never render.
fn validate_invocation(manifest: &ConnectorManifest, errors: &mut Errors) {
    const CONNECT_ONLY: &str = "only runx Connect supplies the paid invocation: bind {\"$runx\": \"invocation\"} only in a hosted operation's declared requests";
    for (name, operation) in &manifest.operations {
        let field = format!("operations.{name}");
        if operation.target.template.binds_invocation() {
            errors.push(&format!("{field}.target"), CONNECT_ONLY);
        }
        if let Some(effect) = &operation.effect
            && std::iter::once(&effect.locator)
                .chain(&effect.each)
                .any(Template::binds_invocation)
        {
            errors.push(&format!("{field}.effect"), CONNECT_ONLY);
        }
        if operation.hosted_http().is_none()
            && implementation_templates(manifest, operation).any(Template::binds_invocation)
        {
            errors.push(&format!("{field}.implementation"), CONNECT_ONLY);
        }
    }
}

fn binds_item(request: &HttpRequest) -> bool {
    request.templates().any(Template::binds_item)
}

fn binds_parameter(request: &HttpRequest) -> bool {
    request.templates().any(Template::binds_parameter)
}

/// How each credential authenticates: a declared credential, presented by
/// fields its declaration names, to hosts inside its audience.
fn validate_auth(
    auth: &ConnectorAuth,
    presentations: &BTreeMap<CredentialName, HttpAuth>,
    field: &str,
    hosts: &[&str],
    errors: &mut Errors,
) {
    for (credential, presentation) in presentations {
        let field = format!("{field}.{credential}");
        if let HttpAuth::Header { name, .. } = presentation
            && !credential_header_name(name.as_str())
        {
            errors.push(&field, "a credential header must be a valid HTTP token and cannot control the transport or session");
        }
        let Some(declaration) = auth.credentials.get(credential) else {
            errors.push(&field, format!("credential {credential} is not declared"));
            continue;
        };
        for name in presentation.fields() {
            if !declaration
                .fields
                .iter()
                .any(|declared| declared.name.as_str() == name)
            {
                errors.push(
                    &field,
                    format!("credential {credential} declares no field {name}"),
                );
            }
        }
        for host in hosts {
            if !declaration
                .audience
                .iter()
                .any(|audience| audience.as_str() == *host)
            {
                errors.push(
                    &field,
                    format!("host {host} is outside credential {credential}'s audience"),
                );
            }
        }
    }
}

/// Headers that frame the message, route it or hold the connection. The
/// transport sets them from the request and its destination, so no manifest
/// does: not a request template, a session header or a credential.
const TRANSPORT_HEADERS: &[&str] = &[
    "host",
    "connection",
    "keep-alive",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "content-length",
    "expect",
];

fn transport_header(name: &str) -> bool {
    TRANSPORT_HEADERS
        .iter()
        .any(|reserved| name.eq_ignore_ascii_case(reserved))
}

/// Headers the MCP protocol sets on a session's requests: the session, its
/// version and event stream, and the standard request headers (method, name
/// and each promoted parameter). No manifest sets them.
fn mcp_protocol_header(name: &str) -> bool {
    const PARAMETER: &str = "mcp-param-";
    [
        "mcp-session-id",
        "mcp-protocol-version",
        "last-event-id",
        "mcp-method",
        "mcp-name",
    ]
    .iter()
    .any(|reserved| name.eq_ignore_ascii_case(reserved))
        || name
            .get(..PARAMETER.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(PARAMETER))
}

fn credential_header_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte))
        && !transport_header(name)
        && !mcp_protocol_header(name)
        && ![
            "accept",
            "content-type",
            "cookie",
            "set-cookie",
            "proxy-authorization",
            "proxy-authenticate",
        ]
        .iter()
        .any(|reserved| name.eq_ignore_ascii_case(reserved))
}

/// A template sets neither what the transport sets nor what a credential's
/// presentation owns.
fn validate_template_headers(
    auth: &BTreeMap<CredentialName, HttpAuth>,
    headers: &BTreeMap<String, Template>,
    field: &str,
    errors: &mut Errors,
) {
    for name in headers.keys() {
        if transport_header(name) {
            errors.push(
                &format!("{field}.{name}"),
                "the transport sets this header from the request and its destination",
            );
        }
        if ["authorization", "proxy-authorization", "cookie"]
            .iter()
            .any(|reserved| name.eq_ignore_ascii_case(reserved))
            || auth
                .values()
                .filter_map(HttpAuth::header_name)
                .any(|header| name.eq_ignore_ascii_case(header))
        {
            errors.push(&format!("{field}.{name}"), "authentication headers are owned by credential presentation, not request templates");
        }
    }
}

/// Whether `each` names an argument array the input schema bounds, so the
/// contract, not the runtime, says how many requests one call sends.
fn bounded_argument_array(schema: Option<&JsonObject>, each: &Template) -> bool {
    let (Some(schema), Some(pointer)) = (schema, each.argument_pointer()) else {
        return false;
    };
    let Some(path) = pointer.strip_prefix('/') else {
        return false;
    };
    let mut node = schema;
    for segment in path.split('/') {
        let segment = segment.replace("~1", "/").replace("~0", "~");
        let Some(next) = node
            .get("properties")
            .and_then(JsonValue::as_object)
            .and_then(|properties| properties.get(&segment))
            .and_then(JsonValue::as_object)
        else {
            return false;
        };
        node = next;
    }
    node.get("type").and_then(JsonValue::as_str) == Some("array")
        && node
            .get("maxItems")
            .is_some_and(|max| matches!(max, JsonValue::Number(_)))
}

fn validate_http(
    manifest: &ConnectorManifest,
    field: &str,
    operation: &ConnectorOperation,
    http: &HttpImplementation,
    errors: &mut Errors,
) {
    let field = format!("{field}.implementation.http");
    let read = operation.access() == OperationAccess::Read;
    for request in http
        .requests
        .iter()
        .chain(http.preconditions.iter().map(|readback| &readback.request))
        .chain(http.readbacks.iter().map(|readback| &readback.request))
    {
        validate_template_headers(
            &http.auth,
            &request.headers,
            &format!("{field}.{}.headers", request.id),
            errors,
        );
    }
    for host in http.allowed_hosts.iter() {
        let domain = host.as_str().strip_prefix("*.").unwrap_or(host.as_str());
        if domain.contains('*') || domain.split('.').filter(|label| !label.is_empty()).count() < 2 {
            errors.push(
                &format!("{field}.allowed_hosts"),
                format!("{host} is not a host or *.domain under a registrable domain"),
            );
        }
    }
    if http.allowed_hosts.len() > MAX_HTTP_HOSTS {
        errors.push(
            &format!("{field}.allowed_hosts"),
            format!("at most {MAX_HTTP_HOSTS} hosts"),
        );
    }
    if http.result_paths.is_empty() {
        errors.push(
            &format!("{field}.result_paths"),
            "say where each result field comes from; only an operation with one result field takes the whole answer",
        );
    }
    if http.result_paths.len() > MAX_HTTP_RESULT_PATHS {
        errors.push(
            &format!("{field}.result_paths"),
            format!("at most {MAX_HTTP_RESULT_PATHS} result paths"),
        );
    }
    let mut ids = BTreeSet::new();
    for request in http.requests.iter() {
        if request.id.as_str() == "write" {
            errors.push(
                &format!("{field}.requests"),
                "request id write is reserved for readbacks",
            );
        }
        for named in request.responses() {
            if !ids.contains(named) {
                errors.push(
                    &format!("{field}.requests"),
                    format!(
                        "request {} reads the answer of {named}, which is not an earlier request",
                        request.id
                    ),
                );
            }
        }
        if !ids.insert(request.id.as_str()) {
            errors.push(
                &format!("{field}.requests"),
                format!(
                    "duplicate request id {}; name each request when an exchange has more than one",
                    request.id
                ),
            );
        }
        if read && !matches!(request.method, HttpMethod::Get | HttpMethod::Post) {
            errors.push(
                &format!("{field}.requests"),
                "a read issues GET or POST requests only",
            );
        }
        let binds_item = binds_item(request);
        if let Some(each) = &request.each
            && !bounded_argument_array(operation.input_schema.as_ref(), each)
        {
            errors.push(
                &format!("{field}.requests"),
                format!("request {} repeats over an $argument array whose input schema declares maxItems", request.id),
            );
        }
        if request.each.is_none() && binds_item {
            errors.push(
                &format!("{field}.requests"),
                format!(
                    "request {} binds $item but does not repeat with each",
                    request.id
                ),
            );
        }
        if let Some(pages) = &request.pages {
            if !read || request.method != HttpMethod::Get || request.each.is_some() {
                errors.push(
                    &format!("{field}.requests"),
                    format!("request {} pages only as a read's single GET", request.id),
                );
            }
            if pages
                .limit
                .as_ref()
                .is_some_and(|limit| limit.binds_exchange() || limit.binds_item())
            {
                errors.push(
                    &format!("{field}.requests"),
                    format!(
                        "request {} takes its page limit from the call, not an answer",
                        request.id
                    ),
                );
            }
        }
    }
    if let Some(rate_limit) = &http.rate_limit
        && !rate_limit
            .reset_header
            .as_str()
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        errors.push(
            &format!("{field}.rate_limit.reset_header"),
            "a header name is a lowercase token",
        );
    }
    if read {
        if http.requests.len() > MAX_HTTP_READ_REQUESTS {
            errors.push(
                &format!("{field}.requests"),
                format!("at most {MAX_HTTP_READ_REQUESTS} requests"),
            );
        }
        if !http.readbacks.is_empty() {
            errors.push(&format!("{field}.readbacks"), "a read has no readbacks");
        }
    } else {
        if http.requests.len() != 1 {
            errors.push(
                &format!("{field}.requests"),
                "a mutation dispatches exactly one provider request",
            );
        }
        let acknowledged = operation.guarantees.readback == ReadbackGuarantee::Unavailable;
        if acknowledged && !http.readbacks.is_empty() {
            errors.push(
                &format!("{field}.readbacks"),
                "a write the provider cannot read back declares no readbacks",
            );
        }
        if !acknowledged && (http.readbacks.is_empty() || http.readbacks.len() > MAX_HTTP_READBACKS)
        {
            errors.push(
                &format!("{field}.readbacks"),
                format!("a mutation needs one to {MAX_HTTP_READBACKS} independent readbacks"),
            );
        }
    }
    // Every request in the exchange has its own id, so one id names one
    // declaration wherever the request is rendered.
    for precondition in &http.preconditions {
        if !precondition.request.responses().is_empty() {
            errors.push(
                &format!("{field}.preconditions"),
                format!(
                    "precondition {} runs before the write and reads no answer",
                    precondition.request.id
                ),
            );
        }
    }
    for readback in http.preconditions.iter().chain(&http.readbacks) {
        if readback
            .request
            .responses()
            .iter()
            .any(|named| *named != "write")
        {
            errors.push(
                &format!("{field}.readbacks"),
                format!(
                    "checking read {} reads only the write's answer",
                    readback.request.id
                ),
            );
        }
        if readback.request.each.is_some() || readback.request.pages.is_some() {
            errors.push(
                &format!("{field}.readbacks"),
                "a precondition or readback sends one request",
            );
        }
        if readback.matches.is_empty() && readback.expected_status.is_none() {
            errors.push(
                &format!("{field}.readbacks"),
                format!(
                    "checking read {} must match a value or expect a status",
                    readback.request.id
                ),
            );
        }
        if readback.request.method != HttpMethod::Get {
            errors.push(
                &format!("{field}.readbacks"),
                format!("checking read {} must be a GET", readback.request.id),
            );
        }
        if readback.request.id.as_str() == "write" || !ids.insert(readback.request.id.as_str()) {
            errors.push(
                &format!("{field}.readbacks"),
                format!(
                    "duplicate request id {}; name each request when an exchange has more than one",
                    readback.request.id
                ),
            );
        }
    }
    // A repeated write is read back once per item, so only its readbacks may
    // bind the item; a precondition runs once, before any item is sent.
    let repeats = !read && http.requests.iter().any(|request| request.each.is_some());
    let item_bound = |readback: &HttpReadback| {
        binds_item(&readback.request)
            || readback
                .matches
                .iter()
                .any(|binding| matches!(binding.expected, ReadbackExpected::Item { .. }))
    };
    if http.preconditions.iter().any(item_bound) {
        errors.push(
            &format!("{field}.preconditions"),
            "a precondition runs once, before any item, and binds no item",
        );
    }
    if !repeats && http.readbacks.iter().any(item_bound) {
        errors.push(
            &format!("{field}.readbacks"),
            "a readback binds the item only when the write repeats with each",
        );
    }
    for readback in &http.readbacks {
        if readback.expected_status.is_some() && readback.decode.is_some() {
            errors.push(
                &format!("{field}.readbacks"),
                "a status-only readback has no decoder",
            );
        }
    }
    // The instance's credential picks how a request authenticates, so each
    // credential the operation accepts says how, and no other does.
    let accepted = operation.credentials.iter().collect::<BTreeSet<_>>();
    if accepted != http.auth.keys().collect::<BTreeSet<_>>() {
        errors.push(
            &format!("{field}.auth"),
            "auth says how each of the operation's credentials authenticates, and no other",
        );
    }
    for (credential, presentation) in &http.auth {
        validate_body_credential(
            operation,
            http,
            presentation,
            &format!("{field}.auth.{credential}"),
            errors,
        );
    }
    let hosts = http
        .allowed_hosts
        .iter()
        .map(NonEmptyString::as_str)
        .collect::<Vec<_>>();
    validate_auth(
        &manifest.auth,
        &http.auth,
        &format!("{field}.auth"),
        &hosts,
        errors,
    );
}

/// A credential presented in the body is placed at its pointer in every
/// request the implementation sends: each declares a literal JSON object that
/// holds the pointer's parent and never sets the pointer, and no input the
/// operation takes is named by it, so the call can neither supply nor
/// displace the credential.
fn validate_body_credential(
    operation: &ConnectorOperation,
    http: &HttpImplementation,
    presentation: &HttpAuth,
    field: &str,
    errors: &mut Errors,
) {
    let HttpAuth::Body { pointer, .. } = presentation else {
        return;
    };
    let Some(segments) = presentation.body_pointer() else {
        errors.push(
            &format!("{field}.pointer"),
            format!("{pointer} is not a JSON pointer to a member, such as /key"),
        );
        return;
    };
    let Some((last, parent)) = segments.split_last() else {
        return;
    };
    for request in http
        .requests
        .iter()
        .chain(http.preconditions.iter().map(|readback| &readback.request))
        .chain(http.readbacks.iter().map(|readback| &readback.request))
    {
        match request
            .body
            .as_ref()
            .and_then(|body| body.literal_object(parent))
        {
            None => errors.push(
                field,
                format!(
                    "request {} declares no literal JSON object at the parent of {pointer}, so it cannot carry the credential",
                    request.id
                ),
            ),
            Some(object) if object.contains_key(last) => errors.push(
                field,
                format!("request {} already sets {pointer}", request.id),
            ),
            Some(_) => {}
        }
    }
    let named = segments.first().is_some_and(|first| {
        operation
            .input_schema
            .as_ref()
            .and_then(|schema| schema.get("properties"))
            .is_some_and(|properties| match properties {
                runx_contracts::JsonValue::Object(properties) => properties.contains_key(first),
                _ => false,
            })
    });
    if named {
        errors.push(
            field,
            format!("an input named by {pointer} would collide with the credential"),
        );
    }
}

/// Operations that share one MCP tool must each fix the discriminating
/// arguments as literals, and no two may fix the same literals. Otherwise a
/// read could be steered into a write through its input.
fn validate_mcp_multiplexing(manifest: &ConnectorManifest, errors: &mut Errors) {
    let mut by_tool: BTreeMap<
        &str,
        Vec<(
            &OperationName,
            &ConnectorOperation,
            &runx_contracts::connector::McpImplementation,
        )>,
    > = BTreeMap::new();
    for (name, operation) in &manifest.operations {
        if let Some(Implementation::Mcp(mcp)) = &operation.implementation {
            by_tool
                .entry(mcp.tool.as_str())
                .or_default()
                .push((name, operation, mcp));
        }
    }
    for (tool, operations) in by_tool
        .into_iter()
        .filter(|(_, operations)| operations.len() > 1)
    {
        // Steering matters only where a read and a write share a tool.
        if operations
            .iter()
            .all(|(_, operation, _)| operation.access() == OperationAccess::Read)
        {
            continue;
        }
        // A read whose arguments bind nothing always sends the same call, so
        // no input steers it; it needs no discriminator. No two operations
        // may send the same fixed call.
        for (index, (name, _, mcp)) in operations.iter().enumerate() {
            for (other_name, _, other) in operations.iter().skip(index + 1) {
                if mcp.arguments.is_literal() && mcp.arguments == other.arguments {
                    errors.push(
                        &format!("operations.{other_name}.implementation.mcp.arguments"),
                        format!("{other_name} and {name} send tool {tool} the same call"),
                    );
                }
            }
        }
        let literals = operations
            .iter()
            .map(|(name, operation, mcp)| {
                (
                    *name,
                    operation.access() == OperationAccess::Read && mcp.arguments.is_literal(),
                    discriminating_fields(operation, &mcp.arguments),
                    mcp.arguments.runx_fields(),
                )
            })
            .collect::<Vec<_>>();
        let discriminators = literals
            .iter()
            .flat_map(|(_, _, fields, _)| fields.keys().copied())
            .collect::<BTreeSet<_>>();
        for (name, _, fields, runx) in &literals {
            for key in &discriminators {
                // runx fills its own provider key; no input steers it.
                if !fields.contains_key(key) && !runx.contains(key) {
                    errors.push(
                        &format!("operations.{name}.implementation.mcp.arguments"),
                        format!("{key} discriminates tool {tool} and must be a literal here"),
                    );
                }
            }
        }
        if discriminators.is_empty() {
            // Nothing tells the calls apart, so only one operation may take
            // input; a read that binds nothing sends its fixed call.
            let steerable = literals
                .iter()
                .filter(|(_, fixed_read, _, _)| !fixed_read)
                .map(|(name, _, _, _)| *name)
                .collect::<Vec<_>>();
            for other_name in steerable.iter().skip(1) {
                errors.push(
                    &format!("operations.{other_name}.implementation.mcp.arguments"),
                    format!(
                        "{other_name} and {} call tool {tool} with nothing to tell them apart",
                        steerable[0]
                    ),
                );
            }
            continue;
        }
        for (index, (name, _, fields, _)) in literals.iter().enumerate() {
            for (other_name, _, other, _) in literals.iter().skip(index + 1) {
                if discriminators
                    .iter()
                    .all(|key| fields.get(key) == other.get(key))
                {
                    errors.push(
                        &format!("operations.{other_name}.implementation.mcp.arguments"),
                        format!("{other_name} and {name} call tool {tool} with the same literals"),
                    );
                }
            }
        }
    }
}

/// The tool arguments an operation fixes for every call: the template's
/// literal members and, where the template passes an input object through
/// whole, the members the input schema requires as a `const`. The kernel
/// validates input against the schema before it renders, so both are fixed.
fn discriminating_fields<'a>(
    operation: &'a ConnectorOperation,
    arguments: &'a Template,
) -> BTreeMap<&'a str, &'a JsonValue> {
    let mut fields = arguments.literal_fields();
    let Some(pointer) = arguments.argument_pointer() else {
        return fields;
    };
    let mut schema = operation.input_schema.as_ref();
    for segment in pointer.split('/').skip(1) {
        schema = schema
            .and_then(|schema| schema.get("properties"))
            .and_then(JsonValue::as_object)
            .and_then(|properties| properties.get(segment))
            .and_then(JsonValue::as_object);
    }
    let Some(schema) = schema else {
        return fields;
    };
    let required = schema
        .get("required")
        .and_then(JsonValue::as_array)
        .map(|required| {
            required
                .iter()
                .filter_map(JsonValue::as_str)
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();
    for (key, property) in schema
        .get("properties")
        .and_then(JsonValue::as_object)
        .into_iter()
        .flatten()
    {
        if let Some(value) = property
            .as_object()
            .and_then(|property| property.get("const"))
            && required.contains(key.as_str())
        {
            fields.insert(key.as_str(), value);
        }
    }
    fields
}

fn local_auth_name(method: LocalAuth) -> &'static str {
    match method {
        LocalAuth::Credentials => "credentials",
        LocalAuth::Session => "session",
        LocalAuth::None => "none",
    }
}

#[cfg(test)]
mod tests;
