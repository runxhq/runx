use std::io::Read;
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use runx_contracts::ProcessStream;

use sha2::{Digest, Sha256};

use super::ProcessSupervisorError;

pub(super) type CaptureHandle = JoinHandle<std::io::Result<CapturedOutput>>;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct CapturedOutput {
    pub(crate) bytes: Vec<u8>,
    pub(crate) truncated: bool,
    pub(crate) total_bytes: u64,
    pub(crate) sha256: String,
}

/// One stream's follower: who is given its output as it is read.
pub(super) type Following = Option<(Arc<dyn super::Follower>, ProcessStream)>;

pub(super) fn capture_pipe<R>(
    pipe: Option<R>,
    context: String,
    output_limit_bytes: usize,
    following: Following,
) -> Result<CaptureHandle, ProcessSupervisorError>
where
    R: Read + Send + 'static,
{
    pipe.map(|reader| capture_stream(reader, output_limit_bytes, following))
        .ok_or_else(|| {
            ProcessSupervisorError::supervise(
                context,
                std::io::Error::other("pipe was not captured"),
            )
        })
}

fn capture_stream<R>(
    mut reader: R,
    output_limit_bytes: usize,
    following: Following,
) -> CaptureHandle
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut captured = Vec::new();
        let mut truncated = false;
        let mut total_bytes = 0_u64;
        let mut digest = Sha256::new();
        let mut buffer = [0_u8; 8192];
        loop {
            let count = reader.read(&mut buffer)?;
            if count == 0 {
                return Ok(CapturedOutput {
                    bytes: captured,
                    truncated,
                    total_bytes,
                    sha256: format!("sha256:{}", runx_contracts::hex_lower(&digest.finalize())),
                });
            }
            if let Some((follower, stream)) = &following {
                follower.output(*stream, &buffer[..count]);
            }
            digest.update(&buffer[..count]);
            total_bytes = total_bytes.saturating_add(u64::try_from(count).unwrap_or(u64::MAX));
            let remaining = output_limit_bytes.saturating_sub(captured.len());
            if remaining > 0 {
                captured.extend_from_slice(&buffer[..count.min(remaining)]);
            }
            if count > remaining {
                truncated = true;
            }
        }
    })
}

pub(super) fn join_capture(
    handle: CaptureHandle,
    context: String,
) -> Result<CapturedOutput, ProcessSupervisorError> {
    match handle.join() {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(source)) => Err(ProcessSupervisorError::supervise(context, source)),
        Err(_) => Err(ProcessSupervisorError::supervise(
            context,
            std::io::Error::other("output reader thread failed"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    #[test]
    fn bounded_capture_retains_a_prefix_but_hashes_the_complete_stream()
    -> Result<(), Box<dyn std::error::Error>> {
        let complete = b"complete process output".to_vec();
        let output = join_capture(
            capture_stream(Cursor::new(complete.clone()), 8, None),
            "capturing test output".to_owned(),
        )?;

        assert_eq!(output.bytes, &complete[..8]);
        assert!(output.truncated);
        assert_eq!(
            output.total_bytes,
            u64::try_from(complete.len()).map_err(|error| error.to_string())?
        );
        assert_eq!(output.sha256, runx_contracts::sha256_prefixed(&complete));
        Ok(())
    }
}
