//! A scripted model on a loopback port: it speaks chat completions with no
//! key, as a local model server does, and answers each request with what a
//! test's or a bench's responder makes of it. It proves the managed agent's
//! machinery with no spend, and it's scripted, never a real model. Only tests
//! and benches enable `scripted-model`, so no build that ships has it.

use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

use serde_json::{Value, json};

/// Read one keyless `POST /v1/chat/completions` and return its JSON body.
///
/// # Errors
///
/// Returns an error when the request ends early, isn't a keyless chat
/// completions post, or its body isn't JSON.
pub fn read_chat_request(stream: &mut TcpStream) -> io::Result<Value> {
    let invalid = |message: &str| io::Error::new(io::ErrorKind::InvalidData, message.to_owned());
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 4096];
    let (header_end, content_length) = loop {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            return Err(invalid("the request ended before its headers"));
        }
        bytes.extend_from_slice(&chunk[..n]);
        if let Some(index) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&bytes[..index]).to_ascii_lowercase();
            if !headers.starts_with("post /v1/chat/completions http/1.1") {
                return Err(invalid("not a chat completions post"));
            }
            if headers.contains("authorization:") {
                return Err(invalid("a keyless local call carries no API key"));
            }
            let length = headers
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .and_then(|value| value.trim().parse::<usize>().ok())
                .ok_or_else(|| invalid("no content length"))?;
            break (index + 4, length);
        }
    };
    while bytes.len() - header_end < content_length {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            return Err(invalid("the request ended before its body"));
        }
        bytes.extend_from_slice(&chunk[..n]);
    }
    serde_json::from_slice(&bytes[header_end..header_end + content_length])
        .map_err(io::Error::other)
}

/// Write `body` as the answer to the request just read.
///
/// # Errors
///
/// Returns an error when the answer can't be written.
pub fn write_chat_response(stream: &mut TcpStream, body: &Value) -> io::Result<()> {
    let body = body.to_string();
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// A scripted model serving until it's finished.
pub struct ScriptedModel {
    endpoint: String,
    done: Arc<AtomicBool>,
    server: JoinHandle<io::Result<Vec<Value>>>,
}

impl ScriptedModel {
    /// Serve each request with `respond`'s answer to it.
    ///
    /// # Errors
    ///
    /// Returns an error when no loopback port can be bound.
    pub fn serve(mut respond: impl FnMut(&Value) -> Value + Send + 'static) -> io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let endpoint = format!("http://{}/v1/chat/completions", listener.local_addr()?);
        let done = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&done);
        let server = std::thread::spawn(move || {
            let mut requests = Vec::new();
            loop {
                let (mut stream, _) = listener.accept()?;
                if stop.load(Ordering::SeqCst) {
                    return Ok(requests);
                }
                let request = read_chat_request(&mut stream)?;
                write_chat_response(&mut stream, &respond(&request))?;
                requests.push(request);
            }
        });
        Ok(Self {
            endpoint,
            done,
            server,
        })
    }

    /// Where it serves.
    #[must_use]
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Stop serving, and return every request it answered.
    ///
    /// # Errors
    ///
    /// Returns an error when the server failed or can't be woken to stop.
    pub fn finish(self) -> io::Result<Vec<Value>> {
        self.done.store(true, Ordering::SeqCst);
        // The server waits in accept; one connection wakes it to stop.
        let address = self.endpoint.trim_start_matches("http://");
        let address = address.split('/').next().unwrap_or(address);
        drop(TcpStream::connect(address)?);
        self.server
            .join()
            .map_err(|_| io::Error::other("the scripted model's thread failed"))?
    }
}

/// A model answer that makes one tool call.
#[must_use]
pub fn tool_call(id: &str, name: &str, arguments: &Value) -> Value {
    json!({"choices":[{"finish_reason":"tool_calls","message":{"tool_calls":[{
        "id": id,
        "type": "function",
        "function": {"name": name, "arguments": arguments.to_string()},
    }]}}]})
}

/// The managed agent's settings for a scripted model at `endpoint`, named
/// `model`, served keyless from this machine.
#[must_use]
pub fn managed_agent_env(endpoint: &str, model: &str) -> Vec<(String, String)> {
    vec![
        (
            "RUNX_AGENT_PROVIDER".to_owned(),
            crate::agent_config::SCRIPTED_PROVIDER.to_owned(),
        ),
        (
            "RUNX_AGENT_DIALECT".to_owned(),
            "chat_completions".to_owned(),
        ),
        ("RUNX_AGENT_MODEL".to_owned(), model.to_owned()),
        ("RUNX_AGENT_AUTH_MODE".to_owned(), "local_none".to_owned()),
        ("RUNX_AGENT_ENDPOINT_URL".to_owned(), endpoint.to_owned()),
    ]
}

/// A scripted reader of `runx/memory`, for a turn offered memory's search
/// and read: it searches for the turn's `question` input, reads the best
/// match by its handle and returns the selected handle or its words, as the
/// completion schema requests, or says
/// memory holds nothing when the search finds nothing. A reader of words,
/// never of meaning.
#[must_use]
pub fn memory_reader(request: &Value) -> Value {
    let messages = request["messages"].as_array().cloned().unwrap_or_default();
    let tool = |ending: &str| {
        request["tools"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|tool| tool["function"]["name"].as_str())
            .find(|name| name.ends_with(ending))
            .unwrap_or(ending)
            .to_owned()
    };
    let selects = request["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|tool| {
            tool["function"]["name"] == "turn_complete"
                && tool["function"]["parameters"]["properties"]
                    .get("handle")
                    .is_some()
        });
    let answer = |text: &str, found: bool| {
        let value = if selects {
            json!({"handle": text, "found": found, "as_of": "", "observations": []})
        } else {
            json!({"answer": text, "found": found})
        };
        tool_call("call-answer", "turn_complete", &value)
    };
    let called = messages
        .iter()
        .rev()
        .find(|message| message["role"] == "assistant")
        .and_then(|message| message["tool_calls"][0]["function"]["name"].as_str())
        .map(str::to_owned);
    let result = messages
        .iter()
        .rev()
        .find(|message| message["role"] == "tool")
        .and_then(|message| message["content"].as_str())
        .and_then(|content| serde_json::from_str::<Value>(content).ok())
        .unwrap_or(Value::Null);
    let output = &result["outcome"]["output"];
    match called {
        None => {
            let question = messages
                .iter()
                .find(|message| message["role"] == "user")
                .and_then(|message| message["content"].as_str())
                .and_then(|prompt| prompt.split_once("\"question\": "))
                .and_then(|(_, rest)| {
                    serde_json::Deserializer::from_str(rest)
                        .into_iter::<String>()
                        .next()
                })
                .and_then(Result::ok)
                .unwrap_or_default();
            tool_call(
                "call-search",
                &tool("__search"),
                &json!({"levels": [{"level": "personal"}], "query": question}),
            )
        }
        Some(name) if name.ends_with("__search") => {
            match output["entries"][0]["data"]["handle"].as_str() {
                Some(handle) => tool_call("call-read", &tool("__read"), &json!({"handle": handle})),
                None => answer("", false),
            }
        }
        Some(name) if name.ends_with("__read") => match output["entry"]["data"]["text"].as_str() {
            Some(text) => answer(
                if selects {
                    output["entry"]["data"]["handle"]
                        .as_str()
                        .unwrap_or_default()
                } else {
                    text
                },
                true,
            ),
            None => answer("", false),
        },
        Some(_) => answer("", false),
    }
}
