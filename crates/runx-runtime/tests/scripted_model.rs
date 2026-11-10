#![cfg(feature = "agent")]

//! A scripted model on a loopback port: it speaks chat completions with no
//! key, as a local model server does, and answers each request with what a
//! test's responder makes of it. It proves the managed agent's machinery with
//! no spend, and it's scripted, never a real model.

use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

use serde_json::Value;

/// Read one keyless `POST /v1/chat/completions` and return its JSON body.
pub(crate) fn read_chat_request(stream: &mut TcpStream) -> io::Result<Value> {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 4096];
    let (header_end, content_length) = loop {
        let n = stream.read(&mut chunk)?;
        assert!(n > 0, "request ended before headers");
        bytes.extend_from_slice(&chunk[..n]);
        if let Some(index) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&bytes[..index]).to_ascii_lowercase();
            assert!(headers.starts_with("post /v1/chat/completions http/1.1"));
            assert!(
                !headers.contains("authorization:"),
                "keyless local call must carry no API key"
            );
            let length = headers
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .and_then(|value| value.trim().parse::<usize>().ok())
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "content length"))?;
            break (index + 4, length);
        }
    };
    while bytes.len() - header_end < content_length {
        let n = stream.read(&mut chunk)?;
        assert!(n > 0, "request ended before body");
        bytes.extend_from_slice(&chunk[..n]);
    }
    serde_json::from_slice(&bytes[header_end..header_end + content_length])
        .map_err(io::Error::other)
}

/// Write `body` as the answer to the request just read.
pub(crate) fn write_chat_response(stream: &mut TcpStream, body: &Value) -> io::Result<()> {
    let body = body.to_string();
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// A scripted model serving until the test finishes it.
pub(crate) struct ScriptedModel {
    endpoint: String,
    done: Arc<AtomicBool>,
    server: JoinHandle<io::Result<Vec<Value>>>,
}

impl ScriptedModel {
    /// Serve each request with `respond`'s answer to it.
    pub(crate) fn serve(
        mut respond: impl FnMut(&Value) -> Value + Send + 'static,
    ) -> io::Result<Self> {
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

    pub(crate) fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Stop serving, and return every request it answered.
    pub(crate) fn finish(self) -> io::Result<Vec<Value>> {
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
pub(crate) fn tool_call(id: &str, name: &str, arguments: &Value) -> Value {
    serde_json::json!({"choices":[{"finish_reason":"tool_calls","message":{"tool_calls":[{
        "id": id,
        "type": "function",
        "function": {"name": name, "arguments": arguments.to_string()},
    }]}}]})
}

/// The managed agent's settings for a scripted model at `endpoint`, named
/// `model`, served keyless from this machine.
pub(crate) fn managed_agent_env(endpoint: &str, model: &str) -> Vec<(String, String)> {
    vec![
        ("RUNX_AGENT_PROVIDER".to_owned(), "local".to_owned()),
        (
            "RUNX_AGENT_DIALECT".to_owned(),
            "chat_completions".to_owned(),
        ),
        ("RUNX_AGENT_MODEL".to_owned(), model.to_owned()),
        ("RUNX_AGENT_AUTH_MODE".to_owned(), "local_none".to_owned()),
        ("RUNX_AGENT_ENDPOINT_URL".to_owned(), endpoint.to_owned()),
    ]
}
