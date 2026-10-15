use std::path::Path;

use super::{ArtifactPageEncoding, LocalArtifactService, MAX_ARTIFACT_PAGE_BYTES};

#[test]
fn volume_independent_artifacts_snapshot_is_immutable_and_pages_are_contiguous()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    std::fs::write(root.path().join("source.txt"), "a🙂bc")?;
    let service = LocalArtifactService::default();
    let artifact = service.admit(
        root.path(),
        Path::new("source.txt"),
        "text/plain; charset=utf-8",
        crate::services::WorkspaceBounds::Operator,
    )?;
    let other_service = LocalArtifactService::default();
    let other_artifact = other_service.admit(
        root.path(),
        Path::new("source.txt"),
        "text/plain; charset=utf-8",
        crate::services::WorkspaceBounds::Operator,
    )?;
    std::fs::write(root.path().join("source.txt"), "replacement")?;

    let first = service.read_page(&artifact.reference, 0, 5, ArtifactPageEncoding::Utf8)?;
    let second = service.read_page(
        &artifact.reference,
        first.next_offset,
        5,
        ArtifactPageEncoding::Utf8,
    )?;

    assert_eq!(format!("{}{}", first.data, second.data), "a🙂bc");
    assert!(second.eof);
    assert_eq!(first.artifact.whole_digest, artifact.whole_digest);
    assert_eq!(artifact.whole_digest, other_artifact.whole_digest);
    assert!(!artifact.reference.contains("source.txt"));
    assert_ne!(artifact.reference, other_artifact.reference);
    assert!(
        other_service
            .read_page(&artifact.reference, 0, 5, ArtifactPageEncoding::Utf8)
            .is_err()
    );
    Ok(())
}

#[test]
fn volume_independent_artifacts_json_array_pages_preserve_every_record_once()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    std::fs::write(
        root.path().join("archive.js"),
        "window.YTD.items.part0 = [\n{\"id\":1},\n{\"id\":2},\n{\"id\":3}\n]",
    )?;
    let service = LocalArtifactService::default();
    let artifact = service.admit(
        root.path(),
        Path::new("archive.js"),
        "application/javascript",
        crate::services::WorkspaceBounds::Operator,
    )?;
    let mut offset = 0;
    let mut records = Vec::new();
    let mut page_count = 0;
    loop {
        let page = service.read_json_array_page(&artifact.reference, offset, 40)?;
        assert_eq!(page.offset, offset);
        assert!(page.next_offset > offset);
        records.extend(page.records);
        page_count += 1;
        if page.eof {
            break;
        }
        offset = page.next_offset;
    }
    assert!(page_count > 1);
    assert_eq!(records, ["{\"id\":1}", "{\"id\":2}", "{\"id\":3}"]);
    Ok(())
}

#[test]
fn volume_independent_artifacts_reject_a_record_before_it_exceeds_the_page_bound()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    std::fs::write(
        root.path().join("archive.js"),
        format!(
            "window.YTD.items.part0 = [{{\"body\":\"{}\"}}]",
            "x".repeat(256)
        ),
    )?;
    let service = LocalArtifactService::default();
    let artifact = service.admit(
        root.path(),
        Path::new("archive.js"),
        "application/javascript",
        crate::services::WorkspaceBounds::Operator,
    )?;

    let error = service
        .read_json_array_page(&artifact.reference, 0, 64)
        .err()
        .ok_or_else(|| {
            std::io::Error::other("one oversized record must fail within the page ceiling")
        })?;

    assert!(error.to_string().contains("64-byte page ceiling"));
    Ok(())
}

#[test]
fn volume_independent_artifacts_bound_whitespace_only_continuation_pages()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    std::fs::write(
        root.path().join("archive.json"),
        format!("[{{\"id\":1}},{}{{\"id\":2}}]", " ".repeat(256)),
    )?;
    let service = LocalArtifactService::default();
    let artifact = service.admit(
        root.path(),
        Path::new("archive.json"),
        "application/json",
        crate::services::WorkspaceBounds::Operator,
    )?;

    let mut offset = 0;
    let mut records = Vec::new();
    let mut pages = 0;
    loop {
        let page = service.read_json_array_page(&artifact.reference, offset, 32)?;
        assert!(page.length <= 32);
        assert!(page.next_offset > offset);
        records.extend(page.records);
        pages += 1;
        if page.eof {
            break;
        }
        offset = page.next_offset;
    }

    assert!(pages > 2);
    assert_eq!(records, ["{\"id\":1}", "{\"id\":2}"]);
    Ok(())
}

#[test]
fn volume_independent_artifacts_allow_pages_beyond_one_mib_up_to_the_runtime_ceiling()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let bytes = vec![b'x'; (1024 * 1024) + 1];
    std::fs::write(root.path().join("large.bin"), &bytes)?;
    let service = LocalArtifactService::default();
    let artifact = service.admit(
        root.path(),
        Path::new("large.bin"),
        "application/octet-stream",
        crate::services::WorkspaceBounds::Operator,
    )?;

    let page = service.read_page(
        &artifact.reference,
        0,
        MAX_ARTIFACT_PAGE_BYTES,
        ArtifactPageEncoding::Base64,
    )?;
    assert_eq!(page.length, bytes.len() as u64);
    assert!(page.eof);

    let error = service
        .read_page(
            &artifact.reference,
            0,
            MAX_ARTIFACT_PAGE_BYTES + 1,
            ArtifactPageEncoding::Base64,
        )
        .err()
        .ok_or_else(|| std::io::Error::other("page above the runtime ceiling must fail"))?;
    assert!(error.to_string().contains("4194304"));
    Ok(())
}
