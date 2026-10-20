//! The `openai_responses` dialect for the shared agent loop: OpenAI's
//! Responses API, the one its current models take function tools through.

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

const MAX_OUTPUT_TOKENS: u32 = 8192;

pub struct OpenAiResponsesCaller<T> {
    transport: T,
    endpoint: String,
    api_key: SecretString,
    model: String,
}

impl<T> OpenAiResponsesCaller<T> {
    pub fn new(transport: T, endpoint: String, api_key: SecretString, model: String) -> Self {
        Self {
            transport,
            endpoint,
            api_key,
            model,
        }
    }

    fn tools_json(tools: &AgentToolNameMap) -> Vec<WireValue> {
        tools
            .definitions()
            .iter()
            .map(|tool| {
                json!({
                    "type": "function",
                    "name": wire_tool_name(&tool.name),
                    "description": tool.description,
                    "parameters": serde_json::to_value(&tool.input_schema).unwrap_or(WireValue::Null),
                    // Strict schemas require every property; runx tool schemas
                    // are validated by the governed executor instead.
                    "strict": false,
                })
            })
            .collect()
    }

    fn parse_tool_uses(
        tools: &AgentToolNameMap,
        body: &str,
    ) -> Result<Vec<AgentToolUse>, RuntimeError> {
        let response: WireValue = serde_json::from_str(body)
            .map_err(|_| provider_failed("openai response is not JSON"))?;
        if response.get("status").and_then(WireValue::as_str) == Some("incomplete") {
            let reason = response
                .pointer("/incomplete_details/reason")
                .and_then(WireValue::as_str)
                .unwrap_or("unknown");
            return Err(provider_failed(format!(
                "openai response was incomplete: {reason}"
            )));
        }
        let output = response
            .get("output")
            .and_then(WireValue::as_array)
            .ok_or_else(|| provider_failed("openai response has no output"))?;
        let mut uses = Vec::new();
        for item in output
            .iter()
            .filter(|item| item.get("type").and_then(WireValue::as_str) == Some("function_call"))
        {
            let id = item
                .get("call_id")
                .and_then(WireValue::as_str)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| provider_failed("openai function call has no call_id"))?;
            let wire_name = item
                .get("name")
                .and_then(WireValue::as_str)
                .ok_or_else(|| provider_failed("openai function call has no name"))?;
            let input = item
                .get("arguments")
                .and_then(WireValue::as_str)
                .and_then(|value| serde_json::from_str::<JsonValue>(value).ok())
                .unwrap_or(JsonValue::Null);
            uses.push(AgentToolUse {
                id: id.to_owned(),
                name: tools
                    .real_name(wire_name)
                    .unwrap_or(UNRECOGNIZED_MODEL_TOOL)
                    .to_owned(),
                input,
                replay: None,
            });
        }
        Ok(uses)
    }
}

fn input_json(transcript: &[AgentTurn]) -> Vec<WireValue> {
    let mut input = Vec::new();
    for turn in transcript {
        match turn {
            AgentTurn::User(content) => input.push(json!({"role": "user", "content": content})),
            AgentTurn::AssistantToolUses(uses) => {
                input.extend(uses.iter().map(|use_| {
                    json!({
                        "type": "function_call",
                        "call_id": use_.id,
                        "name": wire_tool_name(&use_.name),
                        "arguments": serde_json::to_string(&use_.input).unwrap_or_else(|_| "null".to_owned()),
                    })
                }));
            }
            AgentTurn::ToolResults(results) => {
                input.extend(results.iter().map(|result| {
                    json!({
                        "type": "function_call_output",
                        "call_id": result.tool_use_id,
                        "output": result.content,
                    })
                }));
            }
        }
    }
    input
}

impl<T: RuntimeHttpTransport> ModelCaller for OpenAiResponsesCaller<T> {
    fn next_tool_uses(
        &self,
        tools: &AgentToolNameMap,
        transcript: &[AgentTurn],
    ) -> Result<Vec<AgentToolUse>, RuntimeError> {
        let body = json!({
            "model": self.model,
            "input": input_json(transcript),
            "tools": Self::tools_json(tools),
            "tool_choice": "auto",
            "max_output_tokens": MAX_OUTPUT_TOKENS,
            // The loop replays its own transcript; OpenAI keeps nothing.
            "store": false,
        });
        let body = serde_json::to_string(&body)
            .map_err(|error| RuntimeError::json("serializing openai request", error))?;
        let response = self
            .transport
            .send(RuntimeHttpRequest {
                method: HttpMethod::Post,
                url: self.endpoint.clone(),
                headers: vec![
                    RuntimeHttpHeader::new("content-type", "application/json"),
                    RuntimeHttpHeader::new(
                        "authorization",
                        format!("Bearer {}", self.api_key.expose()),
                    ),
                ],
                body: Some(body),
            })
            .map_err(|error| provider_failed(format!("openai request failed: {error}")))?;
        if !(200..300).contains(&response.status) {
            return Err(provider_refused(
                response.status,
                &response.body,
                Some(self.api_key.expose()),
            ));
        }
        Self::parse_tool_uses(tools, &response.body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::agent_loop::AgentToolResult;
    use crate::adapters::agent_tool_definitions::AgentToolDefinition;
    use crate::http::{RuntimeHttpError, RuntimeHttpResponse};
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

    fn stub(status: u16, body: &str) -> StubTransport {
        StubTransport {
            body: body.to_owned(),
            status,
            requests: RefCell::new(Vec::new()),
        }
    }

    fn caller(stub: &StubTransport) -> OpenAiResponsesCaller<&StubTransport> {
        OpenAiResponsesCaller::new(
            stub,
            "https://responses.test/v1/responses".to_owned(),
            SecretString::new("sk-test-key"),
            "gpt".to_owned(),
        )
    }

    fn tools() -> Result<AgentToolNameMap, String> {
        Ok(AgentToolNameMap::new(vec![AgentToolDefinition {
            name: "acme.post".to_owned(),
            description: "Post the bounded message.".to_owned(),
            input_schema: JsonValue::Object(Default::default()),
        }])?)
    }

    #[test]
    fn a_function_call_becomes_a_tool_use_and_its_result_goes_back_by_call_id() -> Result<(), String>
    {
        let stub = stub(
            200,
            r#"{"status":"completed","output":[{"type":"reasoning","summary":[]},{"type":"function_call","id":"fc_1","call_id":"call_1","name":"acme_post","arguments":"{\"text\":\"hi\"}"}]}"#,
        );
        let transcript = [
            AgentTurn::User("post hi".to_owned()),
            AgentTurn::AssistantToolUses(vec![AgentToolUse {
                id: "call_0".to_owned(),
                name: "acme.post".to_owned(),
                input: JsonValue::Null,
                replay: None,
            }]),
            AgentTurn::ToolResults(vec![AgentToolResult {
                tool_use_id: "call_0".to_owned(),
                content: "{\"ok\":true}".to_owned(),
                is_error: false,
            }]),
        ];
        let uses = caller(&stub)
            .next_tool_uses(&tools()?, &transcript)
            .map_err(|error| error.to_string())?;
        assert!(
            uses.len() == 1 && uses[0].id == "call_1" && uses[0].name == "acme.post",
            "got: {uses:?}"
        );

        let sent = stub.requests.borrow();
        let body: WireValue = serde_json::from_str(
            sent.first()
                .and_then(|request| request.body.as_deref())
                .unwrap_or_default(),
        )
        .map_err(|error| error.to_string())?;
        assert_eq!(body["model"], "gpt");
        assert_eq!(body["max_output_tokens"], MAX_OUTPUT_TOKENS);
        assert_eq!(body["store"], false);
        assert_eq!(body["tools"][0]["name"], "acme_post");
        assert_eq!(body["input"][1]["type"], "function_call");
        assert_eq!(body["input"][1]["call_id"], "call_0");
        assert_eq!(body["input"][2]["type"], "function_call_output");
        assert_eq!(body["input"][2]["call_id"], "call_0");
        Ok(())
    }

    #[test]
    fn a_text_only_reply_is_an_empty_turn() -> Result<(), String> {
        let stub = stub(
            200,
            r#"{"status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":"ready"}]}]}"#,
        );
        let uses = caller(&stub)
            .next_tool_uses(&tools()?, &[AgentTurn::User("go".to_owned())])
            .map_err(|error| error.to_string())?;
        assert!(uses.is_empty());
        Ok(())
    }

    #[test]
    fn an_incomplete_response_fails_with_its_reason() -> Result<(), String> {
        let stub = stub(
            200,
            r#"{"status":"incomplete","incomplete_details":{"reason":"max_output_tokens"},"output":[]}"#,
        );
        let result = caller(&stub).next_tool_uses(&tools()?, &[AgentTurn::User("go".to_owned())]);
        assert!(
            matches!(&result, Err(RuntimeError::ModelProvider { detail })
                if detail == "openai response was incomplete: max_output_tokens"),
            "got: {result:?}"
        );
        Ok(())
    }

    #[test]
    fn a_refusal_names_openais_error_without_the_key() -> Result<(), String> {
        let stub = stub(
            401,
            r#"{"error":{"message":"Incorrect API key provided: sk-test-key.","type":"invalid_request_error","code":"invalid_api_key"}}"#,
        );
        let result = caller(&stub).next_tool_uses(&tools()?, &[AgentTurn::User("go".to_owned())]);
        assert!(
            matches!(&result, Err(RuntimeError::ModelProvider { detail })
                if detail == "status 401 invalid_api_key: Incorrect API key provided: [redacted]."),
            "got: {result:?}"
        );
        Ok(())
    }
}
