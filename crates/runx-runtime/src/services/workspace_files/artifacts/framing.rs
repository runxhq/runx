use std::io::{BufReader, Read, Seek, SeekFrom};

use super::super::WorkspaceFileError;

const MAX_JSON_ASSIGNMENT_PREFIX_BYTES: u64 = 64 * 1024;

pub(super) struct JsonArrayFramePage {
    pub(super) records: Vec<String>,
    pub(super) next_offset: u64,
    pub(super) eof: bool,
}

pub(super) fn frame_json_array_page(
    mut file: impl Read + Seek,
    offset: u64,
    maximum: usize,
    maximum_encoded_records: usize,
) -> Result<JsonArrayFramePage, WorkspaceFileError> {
    let page_end = offset.saturating_add(maximum as u64);
    file.seek(SeekFrom::Start(offset))
        .map_err(WorkspaceFileError::SnapshotUnavailable)?;
    let mut reader = BufReader::new(file);
    let mut position = offset;
    if offset == 0 {
        consume_json_array_prefix(&mut reader, &mut position, maximum)?;
    }

    let mut records = Vec::new();
    let mut encoded_records = 2_usize;
    let (next_offset, eof) = loop {
        let (record_start, first) =
            match next_json_array_record(&mut reader, &mut position, page_end)? {
                NextJsonArrayRecord::Record { start, first } => (start, first),
                NextJsonArrayRecord::End => break (position, true),
                NextJsonArrayRecord::PageBoundary => break (position, false),
            };
        let (record, record_end, record_eof) =
            read_json_array_record(&mut reader, &mut position, first, record_start, maximum)?;
        let range_length = record_end.saturating_sub(offset);
        if range_length > maximum as u64 {
            if records.is_empty() {
                return Err(page_ceiling_error(record_start, maximum));
            }
            break (record_start, false);
        }
        let encoded_record = serde_json::to_vec(&record)
            .map_err(|_| WorkspaceFileError::InvalidArtifactFraming {
                offset: record_start,
                message: "framed record could not be encoded for transport".to_owned(),
            })?
            .len();
        let candidate_encoded = encoded_records
            .saturating_add(encoded_record)
            .saturating_add(usize::from(!records.is_empty()));
        if candidate_encoded > maximum_encoded_records {
            if records.is_empty() {
                return Err(WorkspaceFileError::InvalidArtifactFraming {
                    offset: record_start,
                    message: format!(
                        "one framed record exceeds the {maximum_encoded_records}-byte encoded page budget"
                    ),
                });
            }
            break (record_start, false);
        }
        records.push(record);
        encoded_records = candidate_encoded;
        if record_eof || record_end.saturating_sub(offset) >= maximum as u64 {
            break (record_end, record_eof);
        }
    };

    Ok(JsonArrayFramePage {
        records,
        next_offset,
        eof,
    })
}

fn consume_json_array_prefix(
    reader: &mut BufReader<impl Read>,
    position: &mut u64,
    page_maximum: usize,
) -> Result<(), WorkspaceFileError> {
    let mut saw_assignment = false;
    let mut saw_prefix = false;
    loop {
        let byte = read_byte(reader, position)?.ok_or_else(|| {
            WorkspaceFileError::InvalidArtifactFraming {
                offset: *position,
                message: "source ended before a JSON array began".to_owned(),
            }
        })?;
        if *position > MAX_JSON_ASSIGNMENT_PREFIX_BYTES {
            return Err(WorkspaceFileError::InvalidArtifactFraming {
                offset: *position,
                message: "JavaScript assignment prefix exceeds 64 KiB".to_owned(),
            });
        }
        if *position > page_maximum as u64 {
            return Err(page_ceiling_error(*position, page_maximum));
        }
        match byte {
            b'[' if saw_assignment || !saw_prefix => return Ok(()),
            b'[' => {
                return Err(WorkspaceFileError::InvalidArtifactFraming {
                    offset: position.saturating_sub(1),
                    message: "non-JSON prefix must end with an assignment operator".to_owned(),
                });
            }
            b'=' => saw_assignment = true,
            byte if byte.is_ascii_whitespace() => {}
            _ if saw_assignment => {
                return Err(WorkspaceFileError::InvalidArtifactFraming {
                    offset: position.saturating_sub(1),
                    message: "assignment must be followed by a JSON array".to_owned(),
                });
            }
            _ => saw_prefix = true,
        }
    }
}

enum NextJsonArrayRecord {
    Record { start: u64, first: u8 },
    End,
    PageBoundary,
}

fn next_json_array_record(
    reader: &mut BufReader<impl Read>,
    position: &mut u64,
    page_end: u64,
) -> Result<NextJsonArrayRecord, WorkspaceFileError> {
    loop {
        if *position >= page_end {
            return Ok(NextJsonArrayRecord::PageBoundary);
        }
        let Some(byte) = read_byte(reader, position)? else {
            return Err(WorkspaceFileError::InvalidArtifactFraming {
                offset: *position,
                message: "source ended before the JSON array closed".to_owned(),
            });
        };
        match byte {
            b',' => {}
            b']' => return Ok(NextJsonArrayRecord::End),
            byte if byte.is_ascii_whitespace() => {}
            byte => {
                return Ok(NextJsonArrayRecord::Record {
                    start: position.saturating_sub(1),
                    first: byte,
                });
            }
        }
    }
}

fn read_json_array_record(
    reader: &mut BufReader<impl Read>,
    position: &mut u64,
    first: u8,
    record_start: u64,
    maximum: usize,
) -> Result<(String, u64, bool), WorkspaceFileError> {
    let mut bytes = Vec::with_capacity(maximum.min(16 * 1024));
    bytes.push(first);
    let mut depth = usize::from(matches!(first, b'{' | b'['));
    let mut in_string = first == b'"';
    let mut escaped = false;
    loop {
        let Some(byte) = read_byte(reader, position)? else {
            return Err(WorkspaceFileError::InvalidArtifactFraming {
                offset: *position,
                message: "source ended inside a JSON array record".to_owned(),
            });
        };
        if position.saturating_sub(record_start) > maximum as u64 {
            return Err(page_ceiling_error(record_start, maximum));
        }
        if in_string {
            bytes.push(byte);
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => {
                in_string = true;
                bytes.push(byte);
            }
            b'{' | b'[' => {
                depth = depth.saturating_add(1);
                bytes.push(byte);
            }
            b'}' if depth > 0 => {
                depth -= 1;
                bytes.push(byte);
            }
            b']' if depth > 0 => {
                depth -= 1;
                bytes.push(byte);
            }
            b',' if depth == 0 => return framed_record(bytes, *position, false, record_start),
            b']' if depth == 0 => return framed_record(bytes, *position, true, record_start),
            _ => bytes.push(byte),
        }
    }
}

fn framed_record(
    bytes: Vec<u8>,
    record_end: u64,
    eof: bool,
    record_start: u64,
) -> Result<(String, u64, bool), WorkspaceFileError> {
    let value =
        std::str::from_utf8(&bytes).map_err(|_| WorkspaceFileError::InvalidArtifactFraming {
            offset: record_start,
            message: "record is not valid UTF-8".to_owned(),
        })?;
    let value = value.trim();
    if value.is_empty() || serde_json::from_str::<serde_json::Value>(value).is_err() {
        return Err(WorkspaceFileError::InvalidArtifactFraming {
            offset: record_start,
            message: "record is not valid JSON".to_owned(),
        });
    }
    Ok((value.to_owned(), record_end, eof))
}

fn read_byte(
    reader: &mut BufReader<impl Read>,
    position: &mut u64,
) -> Result<Option<u8>, WorkspaceFileError> {
    let mut byte = [0_u8; 1];
    match reader
        .read(&mut byte)
        .map_err(WorkspaceFileError::SnapshotUnavailable)?
    {
        0 => Ok(None),
        _ => {
            *position = position.saturating_add(1);
            Ok(Some(byte[0]))
        }
    }
}

fn page_ceiling_error(offset: u64, maximum: usize) -> WorkspaceFileError {
    WorkspaceFileError::InvalidArtifactFraming {
        offset,
        message: format!("one framed record or prefix exceeds the {maximum}-byte page ceiling"),
    }
}
