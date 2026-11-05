use std::io::Write;

use serde::Serialize;

/// Bound serialized bytes without cloning the input value tree.
pub(crate) fn bounded_json_bytes(
    value: &impl Serialize,
    limit: u64,
) -> Result<Vec<u8>, serde_json::Error> {
    struct BoundedBytes {
        bytes: Vec<u8>,
        limit: u64,
    }
    impl Write for BoundedBytes {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            let length = self.bytes.len().checked_add(bytes.len());
            if length.is_none_or(|length| {
                u64::try_from(length).map_or(true, |length| length > self.limit)
            }) {
                return Err(std::io::Error::other("JSON byte budget exceeded"));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut output = BoundedBytes {
        bytes: Vec::new(),
        limit,
    };
    serde_json::to_writer(&mut output, value)?;
    Ok(output.bytes)
}
