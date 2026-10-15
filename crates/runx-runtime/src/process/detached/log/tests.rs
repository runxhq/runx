use runx_contracts::ProcessStream;

use super::{FRAME_BYTES, HEADER_BYTES, LogRead, LogWriter, SEGMENT_BYTES, SEGMENTS, read};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn text(read: &LogRead) -> Vec<(ProcessStream, String)> {
    read.chunks
        .iter()
        .map(|(stream, bytes)| (*stream, String::from_utf8_lossy(bytes).into_owned()))
        .collect()
}

#[test]
fn output_reads_back_in_order_from_any_cursor_it_returned() -> TestResult {
    let directory = tempfile::tempdir()?;
    let mut log = LogWriter::create(directory.path())?;
    log.append(ProcessStream::Stdout, b"one ")?;
    log.append(ProcessStream::Stdout, b"two\n")?;
    log.append(ProcessStream::Stderr, b"oops\n")?;

    let all = read(directory.path(), 0, 1024)?;
    assert_eq!(
        text(&all),
        [
            (ProcessStream::Stdout, "one two\n".to_owned()),
            (ProcessStream::Stderr, "oops\n".to_owned()),
        ],
        "consecutive frames of one stream join"
    );
    assert_eq!((all.from, all.dropped), (0, 0));

    log.append(ProcessStream::Stdout, b"three\n")?;
    let rest = read(directory.path(), all.next, 1024)?;
    assert_eq!(text(&rest), [(ProcessStream::Stdout, "three\n".to_owned())]);
    let none = read(directory.path(), rest.next, 1024)?;
    assert!(none.chunks.is_empty());
    assert_eq!(none.next, rest.next);
    Ok(())
}

#[test]
fn a_read_stops_near_its_budget_but_always_returns_a_frame() -> TestResult {
    let directory = tempfile::tempdir()?;
    let mut log = LogWriter::create(directory.path())?;
    for _ in 0..4 {
        log.append(ProcessStream::Stdout, &[b'x'; 100])?;
        log.append(ProcessStream::Stderr, &[b'y'; 100])?;
    }
    let first = read(directory.path(), 0, 1)?;
    assert_eq!(text(&first), [(ProcessStream::Stdout, "x".repeat(100))]);
    let next = read(directory.path(), first.next, 250)?;
    assert_eq!(next.chunks.len(), 3);
    Ok(())
}

#[test]
fn old_output_is_dropped_and_a_reader_is_told_how_much() -> TestResult {
    let directory = tempfile::tempdir()?;
    let mut log = LogWriter::create(directory.path())?;
    let frame = vec![b'z'; FRAME_BYTES];
    let frames_per_segment = SEGMENT_BYTES as usize / (HEADER_BYTES + FRAME_BYTES) + 1;
    for _ in 0..frames_per_segment * (SEGMENTS + 2) {
        log.append(ProcessStream::Stdout, &frame)?;
    }
    let segments = std::fs::read_dir(directory.path())?.count();
    assert_eq!(segments, SEGMENTS, "only the newest segments are kept");

    let read = read(directory.path(), 0, 1)?;
    assert!(read.from > 0);
    assert_eq!(
        read.dropped, read.from,
        "everything before the oldest kept is dropped"
    );
    assert_eq!(read.chunks.len(), 1);
    Ok(())
}

#[test]
fn a_cursor_inside_a_frame_is_refused() -> TestResult {
    let directory = tempfile::tempdir()?;
    let mut log = LogWriter::create(directory.path())?;
    log.append(ProcessStream::Stdout, b"hello")?;
    let error = read(directory.path(), 2, 1024).err().ok_or("refused")?;
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    Ok(())
}

#[test]
fn a_frame_still_being_written_is_left_for_a_later_read() -> TestResult {
    let directory = tempfile::tempdir()?;
    let mut log = LogWriter::create(directory.path())?;
    log.append(ProcessStream::Stdout, b"done")?;
    let segment = std::fs::read_dir(directory.path())?
        .next()
        .ok_or("a segment")??
        .path();
    // Half a frame, as a writer leaves it mid-write.
    let mut file = std::fs::OpenOptions::new().append(true).open(segment)?;
    std::io::Write::write_all(&mut file, &[b'o', 0, 0, 0, 9, b'h'])?;

    let read = read(directory.path(), 0, 1024)?;
    assert_eq!(text(&read), [(ProcessStream::Stdout, "done".to_owned())]);
    assert_eq!(read.next, (HEADER_BYTES + 4) as u64);
    Ok(())
}

#[test]
fn an_empty_log_reads_nothing_at_its_cursor() -> TestResult {
    let directory = tempfile::tempdir()?;
    let read = read(&directory.path().join("absent"), 7, 1024)?;
    assert_eq!((read.from, read.next, read.dropped), (7, 7, 0));
    assert!(read.chunks.is_empty());
    Ok(())
}
