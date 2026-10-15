#![allow(clippy::expect_used, clippy::unwrap_used)]

use super::*;

fn validate(yaml: &str) -> Result<ValidatedConnector, Vec<String>> {
    let manifest = parse_connector_manifest(yaml).map_err(|error| vec![error.to_string()])?;
    validate_connector_manifest(manifest)
        .map_err(|errors| errors.into_iter().map(|error| error.to_string()).collect())
}

fn assert_rejects(yaml: &str, expected: &str) {
    let errors = validate(yaml).expect_err("manifest should be rejected");
    assert!(
        errors.iter().any(|error| error.contains(expected)),
        "expected {expected:?} in {errors:?}"
    );
}

const FORGE: &str = r#"
schema: runx.connector.v1
id: forge
name: Forge
description: Issues and pull requests.
runtime_capability: false
auth:
  delegated: {principal: {url: "https://api.example.com/user", id: id, login: login}}
placements:
  hosted: {auth: delegated}
operations:
  issue.read:
    description: Read an issue.
    verb: read
    scopes: [repo.read]
    target: {template: {"$argument": "/repo"}}
    hosted: {provider_scopes: [repo]}
  pullrequest.merge:
    description: Merge a pull request.
    verb: merge
    scopes: [pr.merge]
    target: {template: {"$argument": "/repository"}}
    approval: required
"#;

const NITROSEND: &str = r#"
schema: runx.connector.v1
id: nitrosend
name: Nitrosend
description: Email campaigns and transactional sends.
runtime_capability: false
auth:
  credentials:
    api_key:
      kind: api_key
      fields: [{name: api_key, label: API key}]
      delivery: {env: NITROSEND_API_KEY}
      audience: [api.nitrosend.com]
placements:
  local:
    auth: [credentials]
    mcp_server: {transport: streamable_http, url: "https://api.nitrosend.com/mcp", auth: {api_key: {type: bearer, field: api_key}}}
operations:
  billing.status:
    description: Read billing status.
    verb: read
    scopes: ["nitrosend:read"]
    credentials: [api_key]
    target: {template: "nitrosend://billing"}
    input_schema: {type: object}
    implementation: {mcp: {tool: nitro_manage_billing, arguments: {action: status}, tool_digest: "sha256:0000000000000000000000000000000000000000000000000000000000000000"}}
  billing.checkout:
    description: Start a plan checkout.
    verb: execute
    scopes: ["nitrosend:billing"]
    credentials: [api_key]
    target: {template: "nitrosend://billing"}
    approval: required
    input_schema: {type: object}
    implementation:
      mcp:
        tool: nitro_manage_billing
        arguments: {action: checkout, plan: {"$argument": "/plan"}}
        tool_digest: "sha256:0000000000000000000000000000000000000000000000000000000000000000"
        readback:
          tool: nitro_manage_billing
          arguments: {action: status}
          tool_digest: "sha256:0000000000000000000000000000000000000000000000000000000000000000"
          matches: [{expected: {source: acknowledgement, path: /checkout_id}, observed: /pending_checkout_id}]
"#;

#[test]
fn valid_manifests_produce_digests() {
    let forge = validate(FORGE).expect("forge validates");
    assert_eq!(forge.operation_digests.len(), 2);
    let nitrosend = validate(NITROSEND).expect("nitrosend validates");
    assert_eq!(nitrosend.operation_digests.len(), 2);
}

#[test]
fn reads_cannot_require_approval_or_carry_standing() {
    assert_rejects(
        &FORGE.replace(
            "    hosted: {provider_scopes: [repo]}\n",
            "    approval: required\n    hosted: {provider_scopes: [repo]}\n",
        ),
        "a read cannot require approval",
    );
}

#[test]
fn delegated_placement_needs_provider_scopes() {
    assert_rejects(
        &FORGE.replace("hosted: {provider_scopes: [repo]}", "hosted: {}"),
        "delegated placement needs the provider's scopes",
    );
}

#[test]
fn multiplexed_mcp_tools_need_literal_distinct_discriminators() {
    assert_rejects(
        &NITROSEND.replace(
            "arguments: {action: checkout, plan: {\"$argument\": \"/plan\"}}",
            "arguments: {action: {\"$argument\": \"/action\"}, plan: {\"$argument\": \"/plan\"}}",
        ),
        "action discriminates tool nitro_manage_billing",
    );
    assert_rejects(
        &NITROSEND.replace(
            "arguments: {action: checkout, plan:",
            "arguments: {action: status, plan:",
        ),
        "with the same literals",
    );
    // A read that binds nothing cannot be steered, so it may share a tool
    // whose writes take every argument from input; two operations that both
    // take input with nothing to tell them apart may not.
    let unswitched = NITROSEND
        .replace(
            "arguments: {action: status}, tool_digest",
            "arguments: {}, tool_digest",
        )
        .replace(
            "arguments: {action: checkout, plan: {\"$argument\": \"/plan\"}}",
            "arguments: {plan: {\"$argument\": \"/plan\"}}",
        )
        .replace(
            "          arguments: {action: status}\n",
            "          arguments: {}\n",
        );
    validate(&unswitched).expect("a fixed read beside an input-taking write validates");
    assert_rejects(
        &unswitched.replace(
            "arguments: {}, tool_digest",
            "arguments: {account: {\"$argument\": \"/account\"}}, tool_digest",
        ),
        "with nothing to tell them apart",
    );
    // A tool that takes its arguments whole is told apart by what the input
    // schema requires as a const, which the kernel checks before it sends.
    let passed_through = NITROSEND
        .replace(
            "    input_schema: {type: object}\n    implementation:\n      mcp:\n        tool: nitro_manage_billing\n        arguments: {action: checkout, plan: {\"$argument\": \"/plan\"}}",
            "    input_schema: {type: object, required: [arguments], properties: {arguments: {type: object, required: [action], properties: {action: {const: checkout}}}}}\n    implementation:\n      mcp:\n        tool: nitro_manage_billing\n        arguments: {\"$argument\": \"/arguments\"}",
        );
    validate(&passed_through).expect("a schema-pinned discriminator validates");
    assert_rejects(
        &passed_through.replace("action: {const: checkout}", "action: {type: string}"),
        "action discriminates tool nitro_manage_billing",
    );
}

const PLAIN_READ: &str = r#"
schema: runx.connector.v1
id: example
name: Example
description: Example API.
runtime_capability: false
auth: {none: {}}
placements: {local: {auth: [none]}}
operations:
  item.read:
    description: Read one item.
    verb: read
    scopes: [item.read]
    target: {template: "https://api.example.com/items"}
    result_fields: [item]
    implementation:
      http:
        allowed_hosts: [api.example.com]
        requests:
          - method: GET
            url: "https://api.example.com/item"
"#;

fn header_read() -> String {
    PLAIN_READ
        .replace("auth: {none: {}}", "auth:\n  credentials:\n    key:\n      kind: api_key\n      fields: [{name: api_key}]\n      delivery: {env: EXAMPLE_KEY}\n      audience: [api.example.com]")
        .replace("placements: {local: {auth: [none]}}", "placements:\n  local: {auth: [credentials]}\n  hosted: {auth: credential, credential: key}")
        .replace("    scopes: [item.read]", "    scopes: [item.read]\n    credentials: [key]\n    hosted: {}")
        .replace("        requests:", "        auth: {key: {type: header, name: x-api-key, field: api_key}}\n        requests:")
}

#[test]
fn stored_keys_present_only_declared_headers_on_both_placements() {
    validate(&header_read()).expect("local and hosted header presentations validate");
    for name in [
        "host",
        "Content-Length",
        "cookie",
        "Connection",
        "Proxy-Connection",
        "Mcp-Session-Id",
        "x invalid",
        "x\\nkey",
    ] {
        assert_rejects(
            &header_read().replace("name: x-api-key", &format!("name: '{name}'")),
            "credential header",
        );
    }
    assert_rejects(
        &header_read().replace("field: api_key", "field: missing"),
        "declares no field",
    );
    assert_rejects(
        &header_read().replace(
            "audience: [api.example.com]",
            "audience: [elsewhere.example.com]",
        ),
        "audience",
    );
    assert_rejects(
        &header_read().replace(
            "            url:",
            "            headers: {X-API-Key: caller-controlled}\n            url:",
        ),
        "credential presentation",
    );
    // What routes, frames or holds the connection is the transport's, in any
    // case and wherever a manifest would set it.
    for name in [
        "Host",
        "connection",
        "Transfer-Encoding",
        "Proxy-Connection",
    ] {
        assert_rejects(
            &header_read().replace(
                "            url:",
                &format!("            headers: {{{name}: {{\"$argument\": \"/routing\"}}}}\n            url:"),
            ),
            "the transport sets this header",
        );
    }
}

#[test]
fn mcp_uses_the_same_header_contract_and_rejects_session_overrides() {
    let header = NITROSEND.replace(
        "type: bearer, field: api_key",
        "type: header, name: x-api-key, field: api_key",
    );
    validate(&header).expect("MCP accepts a credential header");
    assert_rejects(&header.replace("url: \"https://api.nitrosend.com/mcp\", auth:", "url: \"https://api.nitrosend.com/mcp\", headers: {x-api-key: caller-controlled}, auth:"), "credential presentation");
    // The MCP protocol's own headers are the session's: its id and version,
    // and the standard request headers it sets on each call.
    for reserved in [
        "mcp-session-id",
        "Mcp-Name",
        "mcp-method",
        "Mcp-Param-Region",
    ] {
        assert_rejects(
            &header.replace("name: x-api-key", &format!("name: {reserved}")),
            "credential header",
        );
    }
    for reserved in ["mcp-name", "mcp-param-region"] {
        assert_rejects(
            &header.replace(
                "url: \"https://api.nitrosend.com/mcp\", auth:",
                &format!(
                    "url: \"https://api.nitrosend.com/mcp\", headers: {{{reserved}: tenant}}, auth:"
                ),
            ),
            "the runtime does not set itself",
        );
    }
    assert_rejects(
        &header.replace(
            "url: \"https://api.nitrosend.com/mcp\", auth:",
            "url: \"https://api.nitrosend.com/mcp\", headers: {host: elsewhere.example.com}, auth:",
        ),
        "the transport sets this header",
    );
}

const HOOK: &str = r#"
schema: runx.connector.v1
id: hooks
name: Hooks
description: Deliver to a webhook.
runtime_capability: false
auth:
  credentials:
    hook:
      kind: webhook_url
      fields: [{name: url}]
      delivery: {env: HOOK_URL}
      audience: ["*.example.com"]
placements: {local: {auth: [credentials]}}
operations:
  hook.invoke:
    description: Deliver one payload.
    verb: write
    scopes: [hook.invoke]
    credentials: [hook]
    target: {template: "https://hooks.example.com/catch"}
    approval: required
    guarantees: {readback: unavailable}
    input_schema:
      type: object
      required: [payload]
      properties:
        payload: {type: object}
    implementation:
      http:
        allowed_hosts: ["*.example.com"]
        auth: {hook: {type: url, field: url}}
        requests:
          - id: deliver
            method: POST
            url: "https://hooks.example.com/catch"
            body: {"$argument": "/payload"}
        decode: [{op: auto_json}]
        result_paths: {acknowledgement: acknowledgement}
"#;

#[test]
fn a_webhook_write_takes_its_url_as_a_credential_and_is_acknowledged() {
    let validated = validate(HOOK).expect("a credentialed acknowledged write validates");
    assert_eq!(
        validated.manifest.operations["hook.invoke"].finality(),
        "acknowledged"
    );
    // An approval alone never authorizes a write.
    let unauthenticated = HOOK
        .replace(
            "auth:\n  credentials:\n    hook:\n      kind: webhook_url\n      fields: [{name: url}]\n      delivery: {env: HOOK_URL}\n      audience: [\"*.example.com\"]\nplacements: {local: {auth: [credentials]}}",
            "auth: {none: {}}\nplacements: {local: {auth: [none]}}",
        )
        .replace("    credentials: [hook]\n", "")
        .replace("        auth: {hook: {type: url, field: url}}\n", "");
    assert_rejects(
        &unauthenticated,
        "a write to a provider needs a credential, a session or a grant",
    );
    assert_rejects(
        &HOOK.replace(
            "        result_paths:",
            "        readbacks:\n          - request: {id: check, method: GET, url: \"https://hooks.example.com/catch\"}\n            expected_status: 200\n        result_paths:",
        ),
        "a write the provider cannot read back declares no readbacks",
    );
    assert_rejects(
        &HOOK.replace(
            "allowed_hosts: [\"*.example.com\"]",
            "allowed_hosts: [\"*.com\"]",
        ),
        "*.com is not a host or *.domain under a registrable domain",
    );
}

#[test]
fn http_hosts_stay_inside_the_credential_audience() {
    let http = r#"
schema: runx.connector.v1
id: example
name: Example
description: Example API.
runtime_capability: false
auth:
  credentials:
    token:
      kind: api_key
      fields: [{name: token}]
      delivery: {env: EXAMPLE_TOKEN}
      audience: [api.example.com]
placements: {local: {auth: [credentials]}}
operations:
  item.read:
    description: Read an item.
    verb: read
    scopes: [item.read]
    credentials: [token]
    target: {template: {"$argument": "/id"}}
    implementation:
      http:
        allowed_hosts: [evil.example.net]
        auth: {token: {type: bearer}}
        requests: [{id: item, method: GET, url: "https://evil.example.net/items"}]
        decode: [{op: auto_json}]
        result_paths: {item: acknowledgement}
"#;
    assert_rejects(http, "outside credential token's audience");
    let inside = http.replace("evil.example.net", "api.example.com");
    assert_rejects(
        &inside.replace(
            "auth: {token: {type: bearer}}",
            "auth: {token: {type: bearer, field: api_key}}",
        ),
        "credential token declares no field api_key",
    );
    assert_rejects(
        &inside.replace(
            "auth: {token: {type: bearer}}",
            "auth: {token: {type: oauth1}}",
        ),
        "credential token declares no field consumer_key",
    );
    assert_rejects(
        &inside.replace("        auth: {token: {type: bearer}}\n", ""),
        "auth says how each of the operation's credentials authenticates",
    );
}

/// A user credential and an app credential, each presented its own way.
const SOCIAL: &str = r#"
schema: runx.connector.v1
id: social
name: Social
description: Posts and accounts.
runtime_capability: false
auth:
  credentials:
    user:
      kind: oauth1_user
      fields: [{name: consumer_key}, {name: consumer_secret}, {name: access_token}, {name: access_secret}]
      delivery: {env: SOCIAL_USER}
      audience: [api.social.example]
    app:
      kind: bearer_token
      fields: [{name: token}]
      delivery: {env: SOCIAL_APP}
      audience: [api.social.example]
placements: {local: {auth: [credentials]}}
operations:
  account.posts.read:
    description: Read the account's posts.
    verb: read
    scopes: [posts.read]
    credentials: [user, app]
    target: {template: "https://api.social.example"}
    input_schema:
      type: object
      additionalProperties: false
      properties:
        limit: {type: integer, minimum: 1}
    implementation:
      http:
        allowed_hosts: [api.social.example]
        auth: {user: {type: oauth1}, app: {type: bearer}}
        requests:
          - {id: me, method: GET, url: "https://api.social.example/me"}
          - id: posts
            method: GET
            url: {"$concat": ["https://api.social.example/users/", {"$response": "me.json.id"}, "/posts"]}
            pages: {param: cursor, cursor: meta.next, items: data, limit: {"$argument": "/limit"}}
        decode: [{op: auto_json}]
        result_paths: {account: acknowledgement.0, posts: acknowledgement.1.items}
"#;

#[test]
fn each_credential_says_how_it_authenticates() {
    assert!(validate(SOCIAL).is_ok(), "{:?}", validate(SOCIAL).err());
    assert_rejects(
        &SOCIAL.replace(
            "auth: {user: {type: oauth1}, app: {type: bearer}}",
            "auth: {user: {type: oauth1}}",
        ),
        "auth says how each of the operation's credentials authenticates",
    );
}

#[test]
fn a_request_reads_only_earlier_answers() {
    assert_rejects(
        &SOCIAL.replace("me.json.id", "posts.json.id"),
        "reads the answer of posts, which is not an earlier request",
    );
}

#[test]
fn only_a_reads_single_get_pages_and_its_limit_comes_from_the_call() {
    assert_rejects(
        &SOCIAL.replace(
            "items: data, limit: {\"$argument\": \"/limit\"}",
            "items: data, limit: {\"$response\": \"me.json.limit\"}",
        ),
        "takes its page limit from the call",
    );
    assert_rejects(
        &SOCIAL.replace(
            "method: GET\n            url: {\"$concat\"",
            "method: POST\n            url: {\"$concat\"",
        ),
        "pages only as a read's single GET",
    );
}

fn priced(usd_micro: u64) -> String {
    PLAIN_READ.replace(
        "    result_fields: [item]",
        &format!(
            "    result_fields: [item]\n    cost: {{usd_micro: {usd_micro}, per: request, source: docs, source_url: \"https://example.com/pricing\"}}"
        ),
    )
}

fn item_read_digest(yaml: &str) -> Digest {
    let validated = validate(yaml).expect("the manifest validates");
    let name = OperationName::new("item.read").expect("name");
    runx_contracts::connector::operation_digest(
        &validated.manifest.id,
        &name,
        &validated.manifest.operations[&name],
    )
    .expect("digest")
}

#[test]
fn a_price_is_part_of_what_an_operation_means() {
    let unpriced = item_read_digest(PLAIN_READ);
    let priced_at_5000 = item_read_digest(&priced(5000));
    assert_ne!(
        unpriced, priced_at_5000,
        "declaring a price changes the operation"
    );
    assert_ne!(
        priced_at_5000,
        item_read_digest(&priced(6000)),
        "a changed price is confirmed again"
    );
    assert_ne!(
        priced_at_5000,
        item_read_digest(&priced(5000).replace("per: request", "per: success")),
        "a changed basis is confirmed again"
    );
    assert_eq!(
        priced_at_5000,
        item_read_digest(&priced(5000).replace("source: docs", "source: observed")),
        "where the price was read is not what the operation means"
    );
}

#[test]
fn a_price_names_an_https_source_and_one_request() {
    assert_rejects(
        &priced(5000).replace("https://example.com/pricing", "http://example.com/pricing"),
        "a price's source is an https page",
    );
    assert_rejects(
        &SOCIAL.replace(
            "    target: {template: \"https://api.social.example\"}",
            "    target: {template: \"https://api.social.example\"}\n    cost: {usd_micro: 10, per: request, source: docs}",
        ),
        "a priced operation sends one request and cannot page",
    );
}

#[test]
fn a_target_pattern_must_be_a_regular_expression() {
    assert_rejects(
        &FORGE.replace(
            "target: {template: {\"$argument\": \"/repo\"}}",
            "target: {template: {\"$argument\": \"/repo\"}, pattern: \"[a-z\"}",
        ),
        "not a valid regular expression",
    );
}

#[test]
fn a_lone_json_read_takes_its_whole_answer_by_default() {
    let validated = validate(PLAIN_READ).expect("a plain read validates");
    let http = validated.manifest.operations[&OperationName::new("item.read").expect("name")]
        .implementation
        .as_ref()
        .and_then(|implementation| match implementation {
            Implementation::Http(http) => Some(http),
            _ => None,
        })
        .expect("an http implementation");
    assert_eq!(
        http.requests
            .iter()
            .map(|request| request.id.as_str())
            .collect::<Vec<_>>(),
        ["request"]
    );
    assert_eq!(
        http.decode,
        [runx_contracts::connector::DecodeStep::AutoJson]
    );
    assert_eq!(
        http.result_paths,
        BTreeMap::from([("item".to_owned(), NonEmptyString::from("acknowledgement"))])
    );

    assert_rejects(
        &PLAIN_READ.replace("result_fields: [item]", "result_fields: [item, etag]"),
        "say where each result field comes from",
    );
    assert_rejects(
        &PLAIN_READ.replace(
            "            url: \"https://api.example.com/item\"\n",
            "            url: \"https://api.example.com/item\"\n          - method: GET\n            url: \"https://api.example.com/other\"\n",
        ),
        "duplicate request id request; name each request when an exchange has more than one",
    );
}

const REPEATED: &str = r#"
schema: runx.connector.v1
id: example
name: Example
description: Example API.
runtime_capability: false
auth: {none: {}}
placements: {local: {auth: [none]}}
operations:
  items.read:
    description: Read items by id.
    verb: read
    scopes: [item.read]
    target: {template: "https://api.example.com/items"}
    input_schema:
      type: object
      required: [ids]
      properties:
        ids: {type: array, maxItems: 10, items: {type: string}}
    implementation:
      http:
        allowed_hosts: [api.example.com]
        requests:
          - id: item
            each: {"$argument": "/ids"}
            method: GET
            url: {"$concat": ["https://api.example.com/items/", {"$item": ""}]}
        decode: [{op: auto_json}]
        result_paths: {items: acknowledgement}
"#;

#[test]
fn a_repeated_request_is_bounded_by_the_input_schema() {
    validate(REPEATED).expect("an each over a bounded argument array validates");
    assert_rejects(
        &REPEATED.replace("maxItems: 10, ", ""),
        "whose input schema declares maxItems",
    );
    assert_rejects(
        &REPEATED.replace(r#"{"$argument": "/ids"}"#, r#"{"$argument": "/missing"}"#),
        "whose input schema declares maxItems",
    );
}

#[test]
fn input_schemas_are_valid_json_schema() {
    assert_rejects(
        &REPEATED.replace("maxItems: 10", "maxItems: -1"),
        "not a valid JSON Schema",
    );
}

const HOSTED_WRITE: &str = r#"
schema: runx.connector.v1
id: example
name: Example
description: Example API.
runtime_capability: false
auth:
  delegated: {principal: {url: "https://api.example.com/user", id: id, login: login}}
  credentials:
    token:
      kind: api_key
      fields: [{name: token}]
      delivery: {env: EXAMPLE_TOKEN}
      audience: [api.example.com]
placements: {hosted: {auth: delegated}, local: {auth: [credentials]}}
references:
  - {kind: thread, locator: "example://items/[0-9]+"}
operations:
  item.update:
    description: Update an item.
    verb: write
    scopes: [item.write]
    credentials: [token]
    target: {template: {"$argument": "/id"}}
    hosted: {provider_scopes: [items]}
    implementation:
      http:
        allowed_hosts: [api.example.com]
        auth: {token: {type: bearer}}
        requests:
          - {id: update, method: PATCH, url: {"$concat": ["https://api.example.com/items/", {"$argument": "/id"}]}, body: {"$argument": "/patch"}}
        readbacks:
          - request: {id: item, method: GET, url: {"$concat": ["https://api.example.com/items/", {"$argument": "/id"}]}}
            matches: [{expected: {source: argument, path: patch.title}, observed: title}]
        decode: [{op: auto_json}]
        result_paths: {item: acknowledgement}
"#;

/// A delegated grant names its account through a request its hosted
/// operations could make: over https, to a host they reach.
#[test]
fn a_delegated_principal_is_read_where_the_hosted_operations_reach() {
    validate(HOSTED_WRITE).expect("the principal is read from api.example.com");
    assert_rejects(
        &HOSTED_WRITE.replace(
            "https://api.example.com/user",
            "https://accounts.elsewhere.example/user",
        ),
        "host accounts.elsewhere.example is not one the connector's hosted operations reach",
    );
    assert_rejects(
        &HOSTED_WRITE.replace(
            "https://api.example.com/user",
            "http://api.example.com/user",
        ),
        "the principal URL must be https",
    );
    assert_rejects(
        &HOSTED_WRITE.replace(
            r#"delegated: {principal: {url: "https://api.example.com/user", id: id, login: login}}"#,
            "delegated: {}",
        ),
        "principal",
    );
}

#[test]
fn hosted_declared_requests_bind_no_instance_parameter() {
    validate(HOSTED_WRITE).expect("a hosted declared write validates");
    assert_rejects(
        &HOSTED_WRITE.replace(
            r#"{"$concat": ["https://api.example.com/items/", {"$argument": "/id"}]}, body"#,
            r#"{"$concat": ["https://api.example.com/items/", {"$parameter": "/id"}]}, body"#,
        ),
        "without instance parameters",
    );
}

#[test]
fn only_a_hosted_declared_request_binds_the_paid_invocation() {
    const CONNECT_ONLY: &str = "only runx Connect supplies the paid invocation";
    let invocation_query = "            url: \"https://api.example.com/item\"\n            query: {invocation: {\"$runx\": \"invocation\"}}";
    validate(&HOSTED_WRITE.replace(
        r#"body: {"$argument": "/patch"}"#,
        r#"body: {patch: {"$argument": "/patch"}, invocation: {"$runx": "invocation"}}"#,
    ))
    .expect("runx Connect renders a hosted operation's declared requests");
    // The paid invocation is not a key, so a read may send it.
    validate(&header_read().replace(
        "            url: \"https://api.example.com/item\"",
        invocation_query,
    ))
    .expect("a hosted read may send the paid invocation");
    // The kernel renders everything else and never holds the invocation.
    assert_rejects(
        &PLAIN_READ.replace(
            "            url: \"https://api.example.com/item\"",
            invocation_query,
        ),
        CONNECT_ONLY,
    );
    assert_rejects(
        &HOSTED_WRITE.replace(
            r#"target: {template: {"$argument": "/id"}}"#,
            r#"target: {template: {"$runx": "invocation"}}"#,
        ),
        CONNECT_ONLY,
    );
    assert_rejects(
        &HOSTED_WRITE.replace(
            "    hosted: {provider_scopes: [items]}\n",
            "    hosted: {provider_scopes: [items]}\n    effect: {kind: thread, locator: {\"$concat\": [\"example://items/\", {\"$runx\": \"invocation\"}]}}\n",
        ),
        CONNECT_ONLY,
    );
}

#[test]
fn a_checking_read_must_assert_something() {
    assert_rejects(
        &HOSTED_WRITE.replace(
            "            matches: [{expected: {source: argument, path: patch.title}, observed: title}]\n",
            "            matches: []\n",
        ),
        "must match a value or expect a status",
    );
}

#[test]
fn reference_locators_keep_to_syntax_both_engines_read() {
    assert_rejects(
        &HOSTED_WRITE.replace("example://items/[0-9]+", "(?i)example://items/[0-9]+"),
        "use only (?: groups",
    );
    validate(&HOSTED_WRITE.replace("example://items/[0-9]+", "example://(?:items|tasks)/[0-9]+"))
        .expect("a non-capturing group is portable");
}

#[test]
fn an_mcp_credential_must_say_how_it_reaches_the_server() {
    assert_rejects(
        &NITROSEND.replace(", auth: {api_key: {type: bearer, field: api_key}}", ""),
        "the MCP server does not say how credential api_key authenticates",
    );
}

#[test]
fn a_precondition_reads_no_answer() {
    let with_precondition = |url: &str| {
        HOSTED_WRITE.replace(
            "        readbacks:\n",
            &format!(
                "        preconditions:\n          - request: {{id: exists, method: GET, url: {url}}}\n            expected_status: 200\n        readbacks:\n"
            ),
        )
    };
    let argument = r#"{"$concat": ["https://api.example.com/items/", {"$argument": "/id"}]}"#;
    let answer =
        r#"{"$concat": ["https://api.example.com/items/", {"$response": "write.json.id"}]}"#;
    assert!(
        validate(&with_precondition(argument)).is_ok(),
        "{:?}",
        validate(&with_precondition(argument)).err()
    );
    assert_rejects(
        &with_precondition(answer),
        "runs before the write and reads no answer",
    );
}

#[test]
fn a_hosted_request_declares_no_rate_limit() {
    assert_rejects(
        &HOSTED_WRITE.replace(
            "        result_paths: {item: acknowledgement}",
            "        result_paths: {item: acknowledgement}\n        rate_limit: {reset_header: x-ratelimit-reset}",
        ),
        "a hosted request cannot read a reset header",
    );
}

#[test]
fn a_write_declares_what_it_produces_and_only_an_effect_reads_the_result() {
    let with = |references: &str, effect: &str| {
        FORGE
            .replace("placements:", &format!("{references}\nplacements:"))
            .replace(
                "    approval: required\n",
                &format!("    approval: required\n{effect}"),
            )
    };
    let declared =
        "references:\n  - {kind: change_request, locator: \"forge://[^/]+/[^/]+/pulls/[0-9]+\"}";
    let merged = r#"    effect:
      kind: change_request
      locator: {"$concat": ["forge://", {"$argument": "/repository"}, "/pulls/", {"$result": "/number"}]}
"#;
    validate(&with(declared, merged)).expect("a write declares its effect");
    // A plugin-shaped connector declares no references: its effect is typed
    // as declared.
    validate(&with("", merged)).expect("no references to check against");

    assert_rejects(
        &with(declared, &merged.replace("change_request", "thread")),
        "declare no thread locator",
    );
    assert_rejects(
        &with(
            declared,
            "    effect: {kind: change_request, locator: \"forge://a/b/pulls/1\"}\n",
        ),
        "rendered from the call's arguments and result",
    );
    assert_rejects(
        &with(
            declared,
            "    effect: {kind: change_request, locator: {\"$item\": \"/url\"}}\n",
        ),
        "reads $item only under each",
    );
    assert_rejects(
        &with(
            declared,
            "    effect: {kind: change_request, each: {\"$argument\": \"/pulls\"}, locator: {\"$item\": \"/url\"}}\n",
        ),
        "as one $result binding",
    );
    assert_rejects(
        &with(
            declared,
            "    effect: {kind: signal, locator: {\"$result\": \"/url\"}}\n",
        ),
        "unknown variant `signal`",
    );
    // A read produces nothing, and nothing the call renders reads a result
    // it does not have yet.
    assert_rejects(
        &FORGE.replace(
            "    hosted: {provider_scopes: [repo]}\n",
            "    hosted: {provider_scopes: [repo]}\n    effect: {kind: thread, locator: {\"$result\": \"/url\"}}\n",
        ),
        "a read produces nothing",
    );
    assert_rejects(
        &FORGE.replace(
            "target: {template: {\"$argument\": \"/repository\"}}",
            "target: {template: {\"$result\": \"/repository\"}}",
        ),
        "only an effect reads $result",
    );
}

#[test]
fn an_enforced_write_sends_the_key_its_provider_deduplicates_by() {
    let enforced = HOOK.replace(
        "guarantees: {readback: unavailable}",
        "guarantees: {readback: unavailable, provider_idempotency: enforced}",
    );
    assert_rejects(&enforced, "needs the request to send the call's key");
    validate(&enforced.replace(
        r#"body: {"$argument": "/payload"}"#,
        r#"body: {payload: {"$argument": "/payload"}, key: {"$runx": "idempotency_key"}}"#,
    ))
    .expect("a write that sends its key may be enforced");
}

#[test]
fn a_read_neither_claims_deduplication_nor_sends_a_key() {
    assert_rejects(
        &PLAIN_READ.replace(
            "    result_fields: [item]",
            "    guarantees: {provider_idempotency: enforced}\n    result_fields: [item]",
        ),
        "a read changes nothing for its provider to deduplicate",
    );
    assert_rejects(
        &PLAIN_READ.replace(
            "            url: \"https://api.example.com/item\"",
            "            url: \"https://api.example.com/item\"\n            query: {key: {\"$runx\": \"idempotency_key\"}}",
        ),
        "a read has no idempotency key",
    );
}

#[test]
fn a_declared_category_must_be_a_connector_category_of_the_taxonomy() {
    let with_category = |category: &str| {
        FORGE.replace(
            "runtime_capability: false\n",
            &format!("category: {category}\nruntime_capability: false\n"),
        )
    };
    let validated = validate(&with_category("dev-tools")).expect("a connector category validates");
    assert_eq!(
        validated
            .manifest
            .category
            .as_ref()
            .map(|category| category.as_str()),
        Some("dev-tools")
    );
    assert_rejects(&with_category("research"), "not a connector category");
    assert_rejects(
        &with_category("uncategorized"),
        "not in the registry taxonomy",
    );
    assert_rejects(&with_category("payment"), "not in the registry taxonomy");
}

#[test]
fn runtime_capability_is_declared_never_assumed() {
    let undeclared = FORGE.replace("runtime_capability: false\n", "");
    assert!(
        validate(&undeclared).is_err(),
        "a manifest must declare runtime_capability"
    );
    assert!(
        validate(&FORGE.replace("runtime_capability: false", "runtime_capability: \"no\""))
            .is_err()
    );
    assert!(
        !validate(FORGE)
            .expect("forge validates")
            .manifest
            .runtime_capability
    );
}

const INDEXNOW: &str = r#"
schema: runx.connector.v1
id: indexnow
name: IndexNow
description: Tell search engines a site's pages changed.
runtime_capability: false
auth:
  credentials:
    site:
      kind: api_key
      fields: [{name: key}]
      delivery: {env: INDEXNOW_KEY}
      audience: [api.indexnow.org]
placements: {local: {auth: [credentials]}}
operations:
  urls.submit:
    description: Submit changed URLs.
    verb: write
    scopes: [urls.submit]
    credentials: [site]
    target: {template: "https://api.indexnow.org/indexnow"}
    approval: required
    guarantees: {readback: unavailable}
    input_schema:
      type: object
      required: [urls]
      properties:
        urls: {type: array, items: {type: string}}
    implementation:
      http:
        allowed_hosts: [api.indexnow.org]
        auth: {site: {type: body, field: key, pointer: /key}}
        requests:
          - id: submit
            method: POST
            url: "https://api.indexnow.org/indexnow"
            body: {host: example.com, urlList: {"$argument": "/urls"}}
        decode: [{op: auto_json}]
        result_paths: {acknowledgement: acknowledgement}
"#;

#[test]
fn a_body_credential_owns_its_pointer() {
    validate(INDEXNOW).expect("a credential placed in a literal body object validates");
    // The body already sets the member.
    assert_rejects(
        &INDEXNOW.replace(
            "body: {host: example.com,",
            "body: {key: fixed, host: example.com,",
        ),
        "already sets /key",
    );
    // An input could supply it.
    assert_rejects(
        &INDEXNOW.replace(
            "        urls: {type: array, items: {type: string}}\n",
            "        urls: {type: array, items: {type: string}}\n        key: {type: string}\n",
        ),
        "would collide with the credential",
    );
    // The body is computed, so where the credential lands is unknown.
    assert_rejects(
        &INDEXNOW.replace(
            "body: {host: example.com, urlList: {\"$argument\": \"/urls\"}}",
            "body: {\"$argument\": \"/urls\"}",
        ),
        "no literal JSON object",
    );
    assert_rejects(
        &INDEXNOW.replace("pointer: /key", "pointer: key"),
        "is not a JSON pointer",
    );
}

/// A seller connector that provisions an inbox under its own token and holds
/// the inbox's lease for whoever bought it.
const INBOX: &str = r#"
schema: runx.connector.v1
id: acme/inbox
name: Acme Inbox
description: Agent inboxes.
runtime_capability: false
auth:
  credentials:
    service_token:
      kind: api_key
      fields: [{name: token}]
      delivery: {env: ACME_INBOX_TOKEN}
      audience: [inbox.acme.test]
    lease:
      kind: purchase
      fields: [{name: capability}]
      audience: [inbox.acme.test]
placements:
  hosted: {auth: credential, credential: service_token}
operations:
  inbox.provision:
    description: Provision one inbox.
    verb: create
    scopes: [inbox.provision]
    credentials: [service_token]
    target: {template: "acme:inbox"}
    guarantees: {provider_idempotency: inherent, readback: unavailable}
    result_fields: [lease]
    holds:
      action: take
      credential: lease
      target: {"$concat": ["acme:inbox/", {"$result": "/lease/inbox_id"}]}
      expires_at: {"$result": "/lease/expires_at"}
      fields: {capability: {"$response": "write.capability"}}
    hosted: {}
    implementation:
      http:
        allowed_hosts: [inbox.acme.test]
        auth: {service_token: {type: bearer, field: token}}
        requests:
          - {method: POST, url: "https://inbox.acme.test/internal/inboxes", body: {invocation_id: {"$runx": "invocation"}}}
        result_paths: {lease: acknowledgement.lease}
  inbox.delete:
    description: Delete one inbox.
    verb: delete
    scopes: [inbox.delete]
    credentials: [lease]
    target: {template: {"$concat": ["acme:inbox/", {"$argument": "/inbox_id"}]}}
    guarantees: {provider_idempotency: inherent, readback: unavailable}
    input_schema: {type: object, required: [inbox_id], additionalProperties: false, properties: {inbox_id: {type: string}}}
    result_fields: [inbox]
    holds:
      action: release
      credential: lease
      target: {"$concat": ["acme:inbox/", {"$argument": "/inbox_id"}]}
    hosted: {}
    implementation:
      http:
        allowed_hosts: [inbox.acme.test]
        auth: {lease: {type: bearer, field: capability}}
        requests:
          - {method: DELETE, url: {"$concat": ["https://inbox.acme.test/v1/inboxes/", {"$argument": "/inbox_id"}]}}
"#;

#[test]
fn a_purchase_credential_is_taken_from_the_answer_and_held_per_resource() {
    let validated = validate(INBOX).expect("the inbox connector is valid");
    let lease = validated
        .manifest
        .auth
        .credentials
        .iter()
        .find_map(|(name, declaration)| (name.as_str() == "lease").then_some(declaration))
        .expect("the lease credential");
    assert!(lease.is_purchase());
    assert!(lease.env().is_none(), "nothing delivers a held credential");
}

#[test]
fn a_held_value_never_reaches_the_call_s_result() {
    assert_rejects(
        &INBOX.replace(
            "result_paths: {lease: acknowledgement.lease}",
            "result_paths: {lease: acknowledgement}",
        ),
        "the call's result reads the held value",
    );
    assert_rejects(
        &INBOX.replace(
            "        result_paths: {lease: acknowledgement.lease}",
            "        result_paths: {lease: acknowledgement.lease}\n        readbacks:\n          - request: {method: GET, url: \"https://inbox.acme.test/internal/inboxes\"}",
        ),
        "a readback would read the held value again",
    );
}

#[test]
fn a_purchase_credential_is_held_by_runx_alone() {
    assert_rejects(
        &INBOX.replace(
            "      fields: [{name: capability}]\n",
            "      fields: [{name: capability}]\n      delivery: {env: ACME_LEASE}\n",
        ),
        "a purchase credential never leaves runx Connect",
    );
    assert_rejects(
        &INBOX.replace("credential: service_token}", "credential: lease}"),
        "a purchase credential is held per resource",
    );
    assert_rejects(
        &INBOX.replace(
            "    credentials: [lease]\n",
            "    credentials: [lease, service_token]\n",
        ),
        "authorized by its purchase credential alone",
    );
}

#[test]
fn an_operation_on_a_held_resource_names_it_by_its_arguments() {
    assert_rejects(
        &INBOX.replace(
            "    target: {template: {\"$concat\": [\"acme:inbox/\", {\"$argument\": \"/inbox_id\"}]}}\n    guarantees: {provider_idempotency: inherent, readback: unavailable}\n    input_schema",
            "    target: {template: \"acme:inbox\"}\n    guarantees: {provider_idempotency: inherent, readback: unavailable}\n    input_schema",
        ),
        "renders its target from the call's arguments alone",
    );
}

#[test]
fn a_take_fills_every_field_from_what_the_write_returned() {
    assert_rejects(
        &INBOX.replace(
            "fields: {capability: {\"$response\": \"write.capability\"}}",
            "fields: {capability: {\"$result\": \"/lease/capability\"}}",
        ),
        "one $response binding on what the write returned",
    );
    assert_rejects(
        &INBOX.replace(
            "fields: {capability: {\"$response\": \"write.capability\"}}",
            "fields: {}",
        ),
        "fills every field of credential lease",
    );
    assert_rejects(
        &INBOX.replace(
            "      expires_at: {\"$result\": \"/lease/expires_at\"}\n      fields",
            "      fields",
        ),
        "states its expiry as one $result binding",
    );
}

#[test]
fn a_purchase_credential_no_operation_takes_is_refused() {
    assert_rejects(
        &INBOX.replace("      action: take\n", "      action: refresh\n"),
        "filled by an operation that holds it with action take",
    );
}
