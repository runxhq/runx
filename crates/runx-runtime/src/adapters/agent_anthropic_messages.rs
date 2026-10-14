// Module rationale: the dialect keeps request shaping, tool-use parsing, and
// error mapping in one reviewed HTTP boundary.
//! The `anthropic_messages` dialect for the managed-agent loop.
//!
//! Translates the provider-agnostic [`AgentTurn`] transcript into a Messages
//! request and parses `tool_use` content blocks back into
//! [`AgentToolUse`], reusing the runtime HTTP transport rather than adding a new
//! HTTP client. Following the codebase convention for runtime HTTP call sites
//! (for example, the registry client), the wire is built and parsed with
//! `serde_json::Value` and converted to/from the runx `JsonValue` only at the
//! domain boundary.

use runx_contracts::JsonValue;
use serde_json::{Value as WireValue, json};

use super::agent_loop::{
    AgentToolUse, AgentTurn, ModelCaller, UNRECOGNIZED_MODEL_TOOL, provider_failed,
    provider_refused,
};
use super::agent_tool_definitions::{AgentToolNameMap, wire_tool_name};
use crate::RuntimeError;
use crate::credentials::SecretString;
use crate::http::{HttpMethod, RuntimeHttpHeader, RuntimeHttpRequest, RuntimeHttpTransport};

const MESSAGES_VERSION: &str = "2023-06-01";
// Architecture and review agents can return a deeply structured packet in one
// final tool call. A 4K ceiling can truncate that tool input before its root
// object closes, which the loop then misreads as repeated contract failure.
// Eight thousand tokens remains bounded while fitting the declared packets
// shipped by the first-party catalog.
const MAX_TOKENS: u32 = 8192;

/// Calls a Messages endpoint to produce the model's next tool-use requests.
pub struct AnthropicMessagesCaller<T> {
    transport: T,
    url: String,
    api_key: SecretString,
    model: String,
    tools: AgentToolNameMap,
}

impl<T> AnthropicMessagesCaller<T> {
    pub fn new(
        transport: T,
        url: String,
        api_key: SecretString,
        model: String,
        tools: AgentToolNameMap,
    ) -> Self {
        Self {
            transport,
            url,
            api_key,
            model,
            tools,
        }
    }

    fn tools_json(&self) -> Vec<WireValue> {
        self.tools
            .definitions()
            .iter()
            .map(|tool| {
                json!({
                    "name": wire_tool_name(&tool.name),
                    "description": tool.description,
                    "input_schema": to_wire(&tool.input_schema),
                })
            })
            .collect()
    }

    /// Map a wire tool name from the model back to the runx tool ref it was
    /// offered as. runx tool refs are namespaced with a dot (`acme.post`),
    /// which the Anthropic tool-name schema forbids, so they are offered with
    /// dots flattened; recover the real ref before the governed executor sees it.
    fn real_tool_name(&self, wire: &str) -> String {
        self.tools
            .real_name(wire)
            .map_or_else(|| UNRECOGNIZED_MODEL_TOOL.to_owned(), str::to_owned)
    }
}

/// Convert a runx `JsonValue` to a wire `serde_json::Value`. A plain value never
/// fails to serialize; default to null rather than propagate an impossible error.
fn to_wire(value: &JsonValue) -> WireValue {
    serde_json::to_value(value).unwrap_or(WireValue::Null)
}

fn messages_json(transcript: &[AgentTurn]) -> Vec<WireValue> {
    transcript
        .iter()
        .map(|turn| match turn {
            AgentTurn::User(text) => json!({
                "role": "user",
                "content": [{ "type": "text", "text": text }],
            }),
            AgentTurn::AssistantToolUses(uses) => json!({
                "role": "assistant",
                "content": uses
                    .iter()
                    .map(|use_| json!({
                        "type": "tool_use",
                        "id": use_.id,
                        "name": wire_tool_name(&use_.name),
                        "input": to_wire(&use_.input),
                    }))
                    .collect::<Vec<WireValue>>(),
            }),
            AgentTurn::ToolResults(results) => json!({
                "role": "user",
                "content": results
                    .iter()
                    .map(|result| json!({
                        "type": "tool_result",
                        "tool_use_id": result.tool_use_id,
                        "content": result.content,
                        "is_error": result.is_error,
                    }))
                    .collect::<Vec<WireValue>>(),
            }),
        })
        .collect()
}

fn parse_tool_uses(body: &str) -> Result<Vec<AgentToolUse>, RuntimeError> {
    let value: WireValue = serde_json::from_str(body)
        .map_err(|source| RuntimeError::json("parsing anthropic response", source))?;
    let Some(content) = value.get("content").and_then(WireValue::as_array) else {
        return Ok(Vec::new());
    };
    let mut uses = Vec::new();
    for block in content {
        if block.get("type").and_then(WireValue::as_str) != Some("tool_use") {
            continue;
        }
        let (Some(id), Some(name)) = (
            block.get("id").and_then(WireValue::as_str),
            block.get("name").and_then(WireValue::as_str),
        ) else {
            continue;
        };
        let input_wire = block.get("input").cloned().unwrap_or(WireValue::Null);
        let input = serde_json::from_value(input_wire).unwrap_or(JsonValue::Null);
        uses.push(AgentToolUse {
            id: id.to_owned(),
            name: name.to_owned(),
            input,
            replay: None,
        });
    }
    Ok(uses)
}

impl<T: RuntimeHttpTransport> ModelCaller for AnthropicMessagesCaller<T> {
    fn next_tool_uses(&self, transcript: &[AgentTurn]) -> Result<Vec<AgentToolUse>, RuntimeError> {
        let request_body = json!({
            "model": self.model,
            "max_tokens": MAX_TOKENS,
            "messages": messages_json(transcript),
            "tools": self.tools_json(),
        });
        let request_body = serde_json::to_string(&request_body)
            .map_err(|source| RuntimeError::json("serializing anthropic request", source))?;
        let response = self
            .transport
            .send(RuntimeHttpRequest {
                method: HttpMethod::Post,
                url: self.url.clone(),
                headers: vec![
                    RuntimeHttpHeader::new("x-api-key", self.api_key.expose()),
                    RuntimeHttpHeader::new("anthropic-version", MESSAGES_VERSION),
                    RuntimeHttpHeader::new("content-type", "application/json"),
                ],
                body: Some(request_body),
            })
            .map_err(|source| provider_failed(format!("anthropic request failed: {source}")))?;
        if !(200..300).contains(&response.status) {
            return Err(provider_refused(
                response.status,
                &response.body,
                Some(self.api_key.expose()),
            ));
        }
        let mut uses = parse_tool_uses(&response.body)?;
        for use_ in &mut uses {
            use_.name = self.real_tool_name(&use_.name);
        }
        Ok(uses)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::agent_loop::AgentTurn;
    use crate::http::{RuntimeHttpError, RuntimeHttpRequest, RuntimeHttpResponse};
    use std::cell::RefCell;

    struct StubTransport {
        body: String,
        status: u16,
        requests: RefCell<Vec<RuntimeHttpRequest>>,
    }

    impl RuntimeHttpTransport for &StubTransport {
        fn send(
            &self,
            request: RuntimeHttpRequest,
        ) -> Result<RuntimeHttpResponse, RuntimeHttpError> {
            self.requests.borrow_mut().push(request);
            Ok(RuntimeHttpResponse {
                status: self.status,
                body: self.body.clone(),
                headers: Vec::new(),
                body_digest: runx_contracts::sha256_prefixed(self.body.as_bytes()),
                body_bytes: self.body.len(),
            })
        }
    }

    fn caller(stub: &StubTransport) -> Result<AnthropicMessagesCaller<&StubTransport>, String> {
        Ok(AnthropicMessagesCaller::new(
            stub,
            "https://messages.test/v1/messages".to_owned(),
            SecretString::new("key"),
            "claude".to_owned(),
            AgentToolNameMap::new(vec![
                super::super::agent_tool_definitions::AgentToolDefinition {
                    name: "pay".to_owned(),
                    description: "Pay the bounded amount.".to_owned(),
                    input_schema: JsonValue::Object(Default::default()),
                },
            ])?,
        ))
    }

    #[test]
    fn parses_tool_use_from_response() -> Result<(), String> {
        let stub = StubTransport {
            body: r#"{"content":[{"type":"text","text":"thinking"},{"type":"tool_use","id":"tu_1","name":"pay","input":{"amount":50}}]}"#
                .to_owned(),
            status: 200,
            requests: RefCell::new(Vec::new()),
        };
        let result = caller(&stub)?.next_tool_uses(&[AgentTurn::User("buy a quota".to_owned())]);
        assert!(
            matches!(
                &result,
                Ok(uses) if uses.len() == 1 && uses[0].name == "pay" && uses[0].id == "tu_1"
            ),
            "should parse the tool_use block; got: {result:?}"
        );
        let sent = stub.requests.borrow();
        let body: WireValue = serde_json::from_str(
            sent.first()
                .and_then(|request| request.body.as_deref())
                .unwrap_or_default(),
        )
        .map_err(|error| format!("managed-agent request body should be JSON: {error}"))?;
        assert_eq!(body["max_tokens"], MAX_TOKENS);
        assert!(
            sent.len() == 1
                && sent[0].body.as_deref().is_some_and(|body| {
                    body.contains("\"model\":\"claude\"") && body.contains("buy a quota")
                }),
            "request body should carry the model and prompt; got: {:?}",
            sent.first().and_then(|request| request.body.as_deref())
        );
        Ok(())
    }

    #[test]
    fn namespaced_tool_ref_is_flattened_on_the_wire_and_restored_on_the_way_in()
    -> Result<(), String> {
        let stub = StubTransport {
            body: r#"{"content":[{"type":"tool_use","id":"tu_1","name":"acme_post","input":{}}]}"#
                .to_owned(),
            status: 200,
            requests: RefCell::new(Vec::new()),
        };
        let model = AnthropicMessagesCaller::new(
            &stub,
            "https://messages.test/v1/messages".to_owned(),
            SecretString::new("key"),
            "claude".to_owned(),
            AgentToolNameMap::new(vec![
                super::super::agent_tool_definitions::AgentToolDefinition {
                    name: "acme.post".to_owned(),
                    description: "post".to_owned(),
                    input_schema: {
                        let mut schema = runx_contracts::JsonObject::new();
                        schema.insert("type".to_owned(), JsonValue::String("object".to_owned()));
                        JsonValue::Object(schema)
                    },
                },
            ])?,
        );
        let uses = model
            .next_tool_uses(&[AgentTurn::User("go".to_owned())])
            .map_err(|error| format!("call should succeed: {error}"))?;
        // The model's flattened call maps back to the dotted runx tool ref.
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].name, "acme.post");
        // The tool was offered to Anthropic without a dot (the API rejects dots).
        let sent = stub.requests.borrow();
        let body = sent[0].body.as_deref().unwrap_or_default();
        assert!(
            body.contains("\"acme_post\"") && !body.contains("acme.post"),
            "tool must be offered flattened, never dotted; got: {body}"
        );
        Ok(())
    }

    #[test]
    fn unoffered_wire_tool_name_is_replaced_before_leaving_provider_boundary()
    -> Result<(), Box<dyn std::error::Error>> {
        const SENSITIVE_MARKER: &str = "customer_secret_as_tool_name";
        let stub = StubTransport {
            body: format!(
                r#"{{"content":[{{"type":"tool_use","id":"tu_1","name":"{SENSITIVE_MARKER}","input":{{}}}}]}}"#
            ),
            status: 200,
            requests: RefCell::new(Vec::new()),
        };
        let model = AnthropicMessagesCaller::new(
            &stub,
            "https://messages.test/v1/messages".to_owned(),
            SecretString::new("key"),
            "claude".to_owned(),
            AgentToolNameMap::new(vec![
                super::super::agent_tool_definitions::AgentToolDefinition {
                    name: "acme.post".to_owned(),
                    description: "post".to_owned(),
                    input_schema: JsonValue::Object(Default::default()),
                },
            ])?,
        );

        let uses = model.next_tool_uses(&[AgentTurn::User("go".to_owned())])?;

        assert_eq!(uses[0].name, UNRECOGNIZED_MODEL_TOOL);
        assert!(!uses[0].name.contains(SENSITIVE_MARKER));
        Ok(())
    }

    #[test]
    fn non_success_status_is_an_error() -> Result<(), String> {
        let stub = StubTransport {
            body: "rate limited".to_owned(),
            status: 429,
            requests: RefCell::new(Vec::new()),
        };
        let result = caller(&stub)?.next_tool_uses(&[AgentTurn::User("go".to_owned())]);
        assert!(
            matches!(&result, Err(RuntimeError::ModelProvider { detail }) if detail == "status 429"),
            "non-2xx should be an error; got: {result:?}"
        );
        Ok(())
    }

    #[test]
    fn a_refusal_names_the_providers_own_error() -> Result<(), String> {
        let stub = StubTransport {
            body: r#"{"type":"error","error":{"type":"not_found_error","message":"model: claude-retired"}}"#
                .to_owned(),
            status: 404,
            requests: RefCell::new(Vec::new()),
        };
        let result = caller(&stub)?.next_tool_uses(&[AgentTurn::User("go".to_owned())]);
        assert!(
            matches!(&result, Err(RuntimeError::ModelProvider { detail })
                if detail == "status 404 not_found_error: model: claude-retired"),
            "got: {result:?}"
        );
        Ok(())
    }

    #[test]
    fn no_tool_use_blocks_yields_empty() -> Result<(), String> {
        let stub = StubTransport {
            body: r#"{"content":[{"type":"text","text":"done"}]}"#.to_owned(),
            status: 200,
            requests: RefCell::new(Vec::new()),
        };
        let result = caller(&stub)?.next_tool_uses(&[AgentTurn::User("go".to_owned())]);
        assert!(
            matches!(&result, Ok(uses) if uses.is_empty()),
            "no tool_use blocks should yield no uses; got: {result:?}"
        );
        Ok(())
    }

    #[test]
    fn malformed_json_body_is_a_parse_error() -> Result<(), String> {
        let stub = StubTransport {
            body: "not json at all".to_owned(),
            status: 200,
            requests: RefCell::new(Vec::new()),
        };
        let result = caller(&stub)?.next_tool_uses(&[AgentTurn::User("go".to_owned())]);
        assert!(
            result.is_err(),
            "a malformed body must error, not panic; got: {result:?}"
        );
        Ok(())
    }

    #[test]
    fn absent_content_yields_empty() -> Result<(), String> {
        let stub = StubTransport {
            body: "{}".to_owned(),
            status: 200,
            requests: RefCell::new(Vec::new()),
        };
        let result = caller(&stub)?.next_tool_uses(&[AgentTurn::User("go".to_owned())]);
        assert!(
            matches!(&result, Ok(uses) if uses.is_empty()),
            "a response with no content array should yield no uses; got: {result:?}"
        );
        Ok(())
    }

    #[test]
    fn tool_use_block_missing_id_or_name_is_skipped() -> Result<(), String> {
        // One block missing id, one missing name, one well-formed: only the
        // well-formed block survives. A partial block is never half-parsed.
        let stub = StubTransport {
            body: r#"{"content":[
                {"type":"tool_use","name":"no_id","input":{}},
                {"type":"tool_use","id":"no_name","input":{}},
                {"type":"tool_use","id":"ok","name":"pay","input":{"a":1}}
            ]}"#
            .to_owned(),
            status: 200,
            requests: RefCell::new(Vec::new()),
        };
        let result = caller(&stub)?.next_tool_uses(&[AgentTurn::User("go".to_owned())]);
        assert!(
            matches!(&result, Ok(uses) if uses.len() == 1 && uses[0].id == "ok" && uses[0].name == "pay"),
            "blocks missing id or name must be skipped; got: {result:?}"
        );
        Ok(())
    }

    #[test]
    fn tool_use_missing_input_defaults_to_null() -> Result<(), String> {
        let stub = StubTransport {
            body: r#"{"content":[{"type":"tool_use","id":"t","name":"pay"}]}"#.to_owned(),
            status: 200,
            requests: RefCell::new(Vec::new()),
        };
        let result = caller(&stub)?.next_tool_uses(&[AgentTurn::User("go".to_owned())]);
        assert!(
            matches!(&result, Ok(uses) if uses.len() == 1 && matches!(uses[0].input, JsonValue::Null)),
            "a tool_use with no input defaults to a null input; got: {result:?}"
        );
        Ok(())
    }
}
