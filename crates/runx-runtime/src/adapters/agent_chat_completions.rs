//! The `chat_completions` dialect for the shared agent loop: the wire format
//! most providers and local model servers speak.

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

const MAX_TOKENS: u32 = 8192;

pub struct ChatCompletionsCaller<T> {
    transport: T,
    endpoint: String,
    api_key: Option<SecretString>,
    model: String,
    tools: AgentToolNameMap,
}

impl<T> ChatCompletionsCaller<T> {
    pub fn new(
        transport: T,
        endpoint: String,
        api_key: Option<SecretString>,
        model: String,
        tools: AgentToolNameMap,
    ) -> Self {
        Self {
            transport,
            endpoint,
            api_key,
            model,
            tools,
        }
    }

    fn messages_json(&self, transcript: &[AgentTurn]) -> Vec<WireValue> {
        let mut messages = Vec::new();
        for turn in transcript {
            match turn {
                AgentTurn::User(content) => messages.push(json!({"role":"user","content":content})),
                AgentTurn::AssistantToolUses(uses) => {
                    let tool_calls = uses.iter().map(|use_| {
                        let mut call = json!({
                            "id": use_.id,
                            "type": "function",
                            "function": {
                                "name": wire_tool_name(&use_.name),
                                "arguments": serde_json::to_string(&use_.input).unwrap_or_else(|_| "null".to_owned()),
                            }
                        });
                        if let Some(extra) = &use_.replay {
                            call["extra_content"] = extra.clone();
                        }
                        call
                    }).collect::<Vec<_>>();
                    messages
                        .push(json!({"role":"assistant","content":null,"tool_calls":tool_calls}));
                }
                AgentTurn::ToolResults(results) => {
                    for result in results {
                        messages.push(json!({
                            "role":"tool",
                            "tool_call_id":result.tool_use_id,
                            "content":result.content,
                        }));
                    }
                }
            }
        }
        messages
    }

    fn tools_json(&self) -> Vec<WireValue> {
        self.tools.definitions().iter().map(|tool| json!({
            "type":"function",
            "function":{
                "name":wire_tool_name(&tool.name),
                "description":tool.description,
                "parameters":serde_json::to_value(&tool.input_schema).unwrap_or(WireValue::Null),
            }
        })).collect()
    }

    fn parse_tool_uses(&self, body: &str) -> Result<Vec<AgentToolUse>, RuntimeError> {
        let response: WireValue = serde_json::from_str(body)
            .map_err(|error| RuntimeError::json("parsing openai-compatible response", error))?;
        let choices = response
            .get("choices")
            .and_then(WireValue::as_array)
            .filter(|choices| choices.len() == 1)
            .ok_or_else(|| provider_failed("openai-compatible response must contain one choice"))?;
        let choice = &choices[0];
        if choice.get("finish_reason").and_then(WireValue::as_str) == Some("length") {
            return Err(provider_failed("openai-compatible response was truncated"));
        }
        let message = choice
            .get("message")
            .and_then(WireValue::as_object)
            .ok_or_else(|| provider_failed("openai-compatible response has no message"))?;
        let Some(calls) = message.get("tool_calls").filter(|value| !value.is_null()) else {
            return Ok(Vec::new());
        };
        let calls = calls
            .as_array()
            .ok_or_else(|| provider_failed("openai-compatible tool calls must be an array"))?;
        let mut uses = Vec::with_capacity(calls.len());
        for call in calls {
            let id = call
                .get("id")
                .and_then(WireValue::as_str)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| provider_failed("openai-compatible tool call has no id"))?;
            if call.get("type").and_then(WireValue::as_str) != Some("function") {
                return Err(provider_failed(
                    "openai-compatible tool call is not a function",
                ));
            }
            let function = call
                .get("function")
                .and_then(WireValue::as_object)
                .ok_or_else(|| provider_failed("openai-compatible tool call has no function"))?;
            let wire_name = function
                .get("name")
                .and_then(WireValue::as_str)
                .ok_or_else(|| provider_failed("openai-compatible tool call has no name"))?;
            let input = function
                .get("arguments")
                .and_then(WireValue::as_str)
                .and_then(|value| serde_json::from_str::<JsonValue>(value).ok())
                .unwrap_or(JsonValue::Null);
            uses.push(AgentToolUse {
                id: id.to_owned(),
                name: self
                    .tools
                    .real_name(wire_name)
                    .unwrap_or(UNRECOGNIZED_MODEL_TOOL)
                    .to_owned(),
                input,
                // Gemini signs each call and refuses a replay without it.
                replay: call.get("extra_content").cloned(),
            });
        }
        Ok(uses)
    }
}

impl<T: RuntimeHttpTransport> ModelCaller for ChatCompletionsCaller<T> {
    fn next_tool_uses(&self, transcript: &[AgentTurn]) -> Result<Vec<AgentToolUse>, RuntimeError> {
        let body = json!({
            "model":self.model,
            "messages":self.messages_json(transcript),
            "tools":self.tools_json(),
            "tool_choice":"auto",
            // The current name; OpenAI's newer models refuse `max_tokens`.
            "max_completion_tokens":MAX_TOKENS,
        });
        let body = serde_json::to_string(&body)
            .map_err(|error| RuntimeError::json("serializing openai-compatible request", error))?;
        let mut headers = vec![RuntimeHttpHeader::new("content-type", "application/json")];
        if let Some(api_key) = &self.api_key {
            headers.push(RuntimeHttpHeader::new(
                "authorization",
                format!("Bearer {}", api_key.expose()),
            ));
        }
        let response = self
            .transport
            .send(RuntimeHttpRequest {
                method: HttpMethod::Post,
                url: self.endpoint.clone(),
                headers,
                body: Some(body),
            })
            .map_err(|error| {
                provider_failed(format!(
                    "openai-compatible model endpoint is unavailable: {error}"
                ))
            })?;
        if !(200..300).contains(&response.status) {
            return Err(provider_refused(
                response.status,
                &response.body,
                self.api_key.as_ref().map(SecretString::expose),
            ));
        }
        self.parse_tool_uses(&response.body)
    }
}
