//! MCP's stdio transport: newline-delimited JSON-RPC, one message per line.
//!
//! Every read is bounded by `max_message_bytes`, so a hostile or broken peer
//! cannot grow the buffer without limit, and only newly read bytes are scanned
//! for the delimiter.

use std::future::Future;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex};

pub(crate) struct StdioJsonRpcTransport<R, W, Role> {
    read: R,
    write: Arc<tokio::sync::Mutex<W>>,
    buffer: Vec<u8>,
    scanned: usize,
    max_message_bytes: usize,
    error_state: TransportErrorState,
    role: PhantomData<Role>,
}

#[derive(Clone, Default)]
pub(crate) struct TransportErrorState {
    message: Arc<Mutex<Option<String>>>,
}

impl TransportErrorState {
    pub(super) fn record(&self, error: std::io::Error) {
        if let Ok(mut message) = self.message.lock() {
            *message = Some(error.to_string());
        }
    }

    pub(super) fn take(&self) -> Option<String> {
        self.message
            .lock()
            .ok()
            .and_then(|mut message| message.take())
    }
}

impl<R, W, Role> StdioJsonRpcTransport<R, W, Role> {
    pub(crate) fn new(
        read: R,
        write: W,
        max_message_bytes: usize,
        error_state: TransportErrorState,
    ) -> Self {
        Self {
            read,
            write: Arc::new(tokio::sync::Mutex::new(write)),
            buffer: Vec::new(),
            scanned: 0,
            max_message_bytes,
            error_state,
            role: PhantomData,
        }
    }
}

impl<R, W, Role> rmcp::transport::Transport<Role> for StdioJsonRpcTransport<R, W, Role>
where
    R: tokio::io::AsyncRead + Send + Unpin + 'static,
    W: tokio::io::AsyncWrite + Send + Unpin + 'static,
    Role: rmcp::service::ServiceRole,
{
    type Error = std::io::Error;

    fn send(
        &mut self,
        item: rmcp::service::TxJsonRpcMessage<Role>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        let write = Arc::clone(&self.write);
        async move {
            // Compact JSON never contains a raw newline, so one line is one message.
            let mut line = serde_json::to_vec(&item).map_err(std::io::Error::other)?;
            line.push(b'\n');
            let mut write = write.lock().await;
            tokio::io::AsyncWriteExt::write_all(&mut *write, &line).await?;
            tokio::io::AsyncWriteExt::flush(&mut *write).await
        }
    }

    async fn receive(&mut self) -> Option<rmcp::service::RxJsonRpcMessage<Role>> {
        match self.next_message().await {
            Ok(message) => message,
            Err(error) => {
                self.error_state.record(error);
                None
            }
        }
    }

    async fn close(&mut self) -> Result<(), Self::Error> {
        let mut write = self.write.lock().await;
        tokio::io::AsyncWriteExt::shutdown(&mut *write).await
    }
}

impl<R, W, Role> StdioJsonRpcTransport<R, W, Role>
where
    R: tokio::io::AsyncRead + Unpin,
    Role: rmcp::service::ServiceRole,
{
    async fn next_message(
        &mut self,
    ) -> Result<Option<rmcp::service::RxJsonRpcMessage<Role>>, std::io::Error> {
        loop {
            if let Some(end) = self.buffer[self.scanned..]
                .iter()
                .position(|byte| *byte == b'\n')
                .map(|offset| self.scanned + offset)
            {
                let line = self.buffer.drain(..=end).collect::<Vec<_>>();
                self.scanned = 0;
                let line = trim_line(&line);
                if line.is_empty() {
                    continue;
                }
                return decode(line, self.max_message_bytes).map(Some);
            }
            self.scanned = self.buffer.len();
            if self.buffer.len() > self.max_message_bytes {
                return Err(too_large());
            }
            let mut chunk = [0_u8; 8192];
            let read = tokio::io::AsyncReadExt::read(&mut self.read, &mut chunk).await?;
            if read == 0 {
                // A peer may close without a final newline after its last message.
                let line = std::mem::take(&mut self.buffer);
                self.scanned = 0;
                let line = trim_line(&line);
                return if line.is_empty() {
                    Ok(None)
                } else {
                    decode(line, self.max_message_bytes).map(Some)
                };
            }
            self.buffer.extend_from_slice(&chunk[..read]);
        }
    }
}

fn trim_line(line: &[u8]) -> &[u8] {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    line.strip_suffix(b"\r").unwrap_or(line)
}

fn decode<T: serde::de::DeserializeOwned>(
    line: &[u8],
    max_message_bytes: usize,
) -> Result<T, std::io::Error> {
    if line.len() > max_message_bytes {
        return Err(too_large());
    }
    serde_json::from_slice(line)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

fn too_large() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "MCP message exceeded size limit.",
    )
}

#[cfg(test)]
mod tests {
    use rmcp::transport::Transport;
    use tokio::io::AsyncWriteExt;

    use super::{StdioJsonRpcTransport, TransportErrorState};

    const MESSAGE_LIMIT: usize = 4 * 1024 * 1024;

    #[test]
    fn receive_records_malformed_json_as_transport_error() {
        let message = receive_error_message(b"{\n", MESSAGE_LIMIT);

        assert!(
            message
                .as_deref()
                .is_some_and(|message| message.contains("EOF while parsing an object")),
            "{message:?}"
        );
    }

    #[test]
    fn receive_records_oversized_line_as_transport_error() {
        let message = receive_error_message(b"{\"jsonrpc\":\"2.0\"}\n", 8);

        assert_eq!(message.as_deref(), Some("MCP message exceeded size limit."));
    }

    #[test]
    fn receive_bounds_an_unterminated_line() {
        let message = receive_error_message(&[b' '; 64], 8);

        assert_eq!(message.as_deref(), Some("MCP message exceeded size limit."));
    }

    #[test]
    fn receive_reads_newline_delimited_messages() -> Result<(), String> {
        let bytes = b"\r\n{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\r\n";
        tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .build()
            .map_err(|error| error.to_string())?
            .block_on(async move {
                let (mut writer, reader) = tokio::io::duplex(bytes.len());
                writer
                    .write_all(bytes)
                    .await
                    .map_err(|error| error.to_string())?;
                drop(writer);
                let error_state = TransportErrorState::default();
                let mut transport = StdioJsonRpcTransport::new(
                    reader,
                    tokio::io::sink(),
                    MESSAGE_LIMIT,
                    error_state.clone(),
                );
                let first = Transport::<rmcp::RoleClient>::receive(&mut transport).await;
                assert!(first.is_some(), "{:?}", error_state.take());
                let end = Transport::<rmcp::RoleClient>::receive(&mut transport).await;
                assert!(end.is_none());
                Ok(())
            })
    }

    fn receive_error_message(bytes: &[u8], message_limit: usize) -> Option<String> {
        let bytes = bytes.to_vec();
        tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .build()
            .ok()?
            .block_on(async move {
                let (mut writer, reader) = tokio::io::duplex(bytes.len().max(1));
                writer.write_all(&bytes).await.ok()?;
                drop(writer);

                let error_state = TransportErrorState::default();
                let mut transport = StdioJsonRpcTransport::new(
                    reader,
                    tokio::io::sink(),
                    message_limit,
                    error_state.clone(),
                );

                let message = Transport::<rmcp::RoleClient>::receive(&mut transport).await;
                assert!(message.is_none());
                error_state.take()
            })
    }
}
