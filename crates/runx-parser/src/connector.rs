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
    HttpRequest, Implementation, LocalAuth, McpServer, MessageRules, OperationAccess,
    OperationName, OperationVerb, ReadbackExpected, ReadbackGuarantee, ReferenceDeclaration,
    SessionAuth, SessionPrincipal, Template, manifest_digest, operation_digest,
};
use runx_contracts::schema::NonEmptyString;
use runx_contracts::{JsonObject, JsonValue, ReferenceType};

use crate::{ParseError, ValidationError};

const MAX_HTTP_HOSTS: usize = 4;
const MAX_HTTP_RESULT_PATHS: usize = 25;
const MAX_HTTP_READ_REQUESTS: usize = 50;
const MAX_HTTP_READBACKS: usize = 10;

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
    let mut errors = Errors::default();
    validate_auth_and_placements(&manifest, &mut errors);
    for (name, operation) in &manifest.operations {
        validate_operation(&manifest, name, operation, &mut errors);
    }
    validate_mcp_multiplexing(&manifest, &mut errors);
    if let Some(messages) = &manifest.messages {
        validate_messages(messages, &mut errors);
    }
    validate_references(&manifest.references, &mut errors);
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
            // The session's own protocol and credential headers are the
            // runtime's; a manifest adds only headers the server reads.
            const RESERVED: &[&str] = &[
                "authorization",
                "accept",
                "content-type",
                "content-length",
                "host",
                "cookie",
                "connection",
                "mcp-session-id",
                "mcp-protocol-version",
                "last-event-id",
            ];
            for (name, template) in headers {
                let field = format!("placements.local.mcp_server.headers.{name}");
                if name.is_empty()
                    || !name.bytes().all(|byte| {
                        byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'
                    })
                    || RESERVED.contains(&name.as_str())
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
            for (credential, presentation) in server_auth {
                if !matches!(presentation, HttpAuth::Bearer { .. }) {
                    errors.push(
                        &format!("placements.local.mcp_server.auth.{credential}"),
                        "an MCP server authenticates with a bearer token",
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

/// A command session runs one program from `PATH`, stands in for a declared
/// one-field credential, and reads its principal inside that credential's
/// audience.
fn validate_command_session(
    auth: &ConnectorAuth,
    command: &[NonEmptyString],
    credential: &CredentialName,
    principal: &SessionPrincipal,
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
    let host = url::Url::parse(principal.url.as_str())
        .ok()
        .filter(|url| url.scheme() == "https")
        .and_then(|url| url.host_str().map(str::to_ascii_lowercase));
    match host {
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

    if let Some(readback) = &operation.readback {
        match manifest.operations.get(readback) {
            Some(target) if target.access() == OperationAccess::Read => {}
            Some(_) => errors.push(
                &format!("{field}.readback"),
                format!("readback {readback} must be a read"),
            ),
            None => errors.push(
                &format!("{field}.readback"),
                format!("readback {readback} is not an operation on this connector"),
            ),
        }
        if read {
            errors.push(&format!("{field}.readback"), "a read has no readback");
        }
    }
    if let Some(reconcile) = &operation.guarantees.reconcile {
        match manifest.operations.get(reconcile) {
            Some(target) if target.access() == OperationAccess::Read => {}
            _ => errors.push(
                &format!("{field}.guarantees.reconcile"),
                format!("reconcile {reconcile} must name a read operation on this connector"),
            ),
        }
    }
    if read {
        if operation.approval == ApprovalFloor::Required {
            errors.push(
                &format!("{field}.approval"),
                "a read cannot require approval",
            );
        }
        if operation.standing.is_some() {
            errors.push(
                &format!("{field}.standing"),
                "a read has no standing approval",
            );
        }
        if operation.amount == AmountRule::Required {
            errors.push(&format!("{field}.amount"), "a read carries no amount");
        }
    }
    if operation.standing.is_some() && operation.approval != ApprovalFloor::Required {
        errors.push(
            &format!("{field}.standing"),
            "standing approval stands in for a required approval; set approval: required",
        );
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
    // A write to an endpoint that takes no credential, such as a webhook whose
    // URL is the capability, is gated by a person every time instead.
    if !read && manifest.auth.unauthenticated() && operation.approval != ApprovalFloor::Required {
        errors.push(
            &format!("{field}.approval"),
            "a write without authentication needs approval: required",
        );
    }
    let acknowledged = operation.guarantees.readback == ReadbackGuarantee::Unavailable;
    if acknowledged && read {
        errors.push(
            &format!("{field}.guarantees.readback"),
            "only a write can be unavailable for readback",
        );
    }
    if acknowledged && operation.readback.is_some() {
        errors.push(
            &format!("{field}.readback"),
            "an operation the provider cannot read back names no readback",
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
        if placement.auth != HostedAuth::Delegated {
            errors.push(
                &format!("{field}.hosted"),
                "the hosted placement sends declared requests under delegated auth only",
            );
        }
        if http.auth.is_empty()
            || http
                .auth
                .values()
                .any(|auth| !matches!(auth, HttpAuth::Bearer { .. }))
        {
            errors.push(
                &format!("{field}.implementation.http.auth"),
                "the provider gateway presents the delegated token as a bearer",
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

fn validate_messages(messages: &MessageRules, errors: &mut Errors) {
    if let Err(error) = whole_match(messages.actor.as_str()) {
        errors.push(
            "messages.actor",
            format!("not a valid regular expression: {error}"),
        );
    }
    if !messages.mention.as_str().contains("{actor}") {
        errors.push("messages.mention", "the mention must place {actor}");
    }
    for (index, replacement) in messages.inert.iter().enumerate() {
        if let Err(error) = regex::Regex::new(replacement.pattern.as_str()) {
            errors.push(
                &format!("messages.inert[{index}].pattern"),
                format!("not a valid regular expression: {error}"),
            );
        }
    }
}

/// Each declaration names an external kind and a locator pattern that
/// compiles.
fn validate_references(references: &[ReferenceDeclaration], errors: &mut Errors) {
    for (index, reference) in references.iter().enumerate() {
        let external = matches!(
            reference.kind,
            ReferenceType::Thread
                | ReferenceType::Comment
                | ReferenceType::Event
                | ReferenceType::TrackingItem
                | ReferenceType::ChangeRequest
                | ReferenceType::Repository
                | ReferenceType::ExternalUrl
        );
        if !external {
            errors.push(
                &format!("references[{index}].kind"),
                format!(
                    "{} is not an external reference kind",
                    reference.kind.as_str()
                ),
            );
        }
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
    if http.result_paths.is_empty() || http.result_paths.len() > MAX_HTTP_RESULT_PATHS {
        errors.push(
            &format!("{field}.result_paths"),
            format!("one to {MAX_HTTP_RESULT_PATHS} result paths"),
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
                format!("duplicate request id {}", request.id),
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
                format!("duplicate request id {}", readback.request.id),
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
