use std::error::Error;
use std::fs;
use std::io;
use std::path::Path;

use runx_contracts::maturity::MaturityTier;

use super::super::LocalRegistryError;
use super::{STAGING_PREFIX, STAGING_SUFFIX, write_registry_json};
use crate::registry::types::{RegistryPublisher, RegistrySkillVersion, TrustTier};

type TestResult = Result<(), Box<dyn Error>>;

fn record(digest: &str) -> RegistrySkillVersion {
    RegistrySkillVersion {
        skill_id: "acme/echo".to_owned(),
        owner: "acme".to_owned(),
        name: "echo".to_owned(),
        description: Some("Echo a value.".to_owned()),
        category: None,
        source_category: None,
        version: "1.0.0".to_owned(),
        digest: digest.to_owned(),
        signed_manifest: None,
        markdown: "---\nname: echo\n---\nEcho.\n".to_owned(),
        profile_document: None,
        profile_digest: None,
        package_files: Vec::new(),
        package_digest: None,
        paid_listing: None,
        runner_names: vec!["echo".to_owned()],
        source_type: "markdown".to_owned(),
        trust_tier: TrustTier::Community,
        maturity: MaturityTier::default(),
        catalog_kind: None,
        catalog_audience: None,
        catalog_visibility: None,
        source_metadata: None,
        attestations: Vec::new(),
        required_scopes: Vec::new(),
        runtime: None,
        auth: None,
        risk: None,
        runx: None,
        tags: Vec::new(),
        harness_cases: Vec::new(),
        publisher: RegistryPublisher {
            kind: "organization".to_owned(),
            id: "pub_1".to_owned(),
            handle: Some("acme".to_owned()),
            display_name: Some("Acme".to_owned()),
        },
        created_at: "2026-01-01T00:00:00.000Z".to_owned(),
        updated_at: "2026-01-01T00:00:00.000Z".to_owned(),
    }
}

fn entry_names(directory: &Path) -> Result<Vec<String>, Box<dyn Error>> {
    let mut names = fs::read_dir(directory)?
        .map(|entry| Ok(entry?.file_name().to_string_lossy().into_owned()))
        .collect::<Result<Vec<_>, Box<dyn Error>>>()?;
    names.sort();
    Ok(names)
}

fn expect_io_failure(
    result: Result<(), LocalRegistryError>,
    expected_path: &Path,
    expected_kind: io::ErrorKind,
) -> TestResult {
    let Err(error) = result else {
        return Err("expected the registry write to fail".into());
    };
    let LocalRegistryError::Io {
        action,
        path,
        source,
    } = &error
    else {
        return Err(format!("expected an io failure, got {error}").into());
    };
    assert_eq!(*action, "writing");
    assert_eq!(path, expected_path);
    assert_eq!(source.kind(), expected_kind);
    Ok(())
}

#[test]
fn registry_record_survives_a_write_and_read_roundtrip() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("1.0.0.json");
    let version = record("sha256:one");

    write_registry_json(&path, &version, true)?;

    let contents = fs::read_to_string(&path)?;
    assert!(contents.ends_with("}\n"));
    assert_eq!(
        serde_json::from_str::<RegistrySkillVersion>(&contents)?,
        version
    );
    assert_eq!(
        entry_names(directory.path())?,
        vec!["1.0.0.json".to_owned()]
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(&path)?.permissions().mode() & 0o777, 0o600);
    }
    Ok(())
}

/// A reader that opened the record before an update keeps reading the record it
/// opened. Publishing through a rename replaces the directory entry instead of
/// truncating the bytes the reader is still pointing at.
#[cfg(unix)]
#[test]
fn registry_update_keeps_a_retained_handle_on_the_previous_record() -> TestResult {
    use std::io::Read;

    let directory = tempfile::tempdir()?;
    let path = directory.path().join("1.0.0.json");
    write_registry_json(&path, &record("sha256:one"), true)?;
    let published = fs::read_to_string(&path)?;
    let mut retained = fs::File::open(&path)?;
    let split = published
        .find("\"version\"")
        .ok_or("missing version field")?;
    let mut prefix = vec![0; split];
    retained.read_exact(&mut prefix)?;

    let mut updated = record("sha256:two");
    updated.description =
        Some("A longer replacement description changes subsequent field offsets.".to_owned());
    updated.updated_at = "2026-02-02T00:00:00.000Z".to_owned();
    write_registry_json(&path, &updated, false)?;

    let mut observed = String::from_utf8(prefix)?;
    retained.read_to_string(&mut observed)?;
    let retained_record: RegistrySkillVersion = serde_json::from_str(&observed)?;
    assert_eq!(retained_record, record("sha256:one"));
    assert_eq!(observed, published);
    assert_eq!(
        serde_json::from_str::<RegistrySkillVersion>(&fs::read_to_string(&path)?)?,
        updated
    );
    assert_eq!(
        entry_names(directory.path())?,
        vec!["1.0.0.json".to_owned()]
    );
    Ok(())
}

/// Atomic replacement changes the record's directory entry and leaves an
/// existing symbolic-link target unchanged.
#[cfg(unix)]
#[test]
fn registry_update_replaces_a_symlinked_record_instead_of_following_it() -> TestResult {
    let directory = tempfile::tempdir()?;
    let outside = directory.path().join("outside.json");
    fs::write(&outside, "outside\n")?;
    let skill = directory.path().join("skill");
    fs::create_dir(&skill)?;
    let path = skill.join("1.0.0.json");
    std::os::unix::fs::symlink(&outside, &path)?;

    write_registry_json(&path, &record("sha256:one"), false)?;

    assert_eq!(fs::read_to_string(&outside)?, "outside\n");
    assert!(!fs::symlink_metadata(&path)?.file_type().is_symlink());
    assert_eq!(entry_names(&skill)?, vec!["1.0.0.json".to_owned()]);
    Ok(())
}

#[test]
fn registry_no_clobber_publish_preserves_the_existing_record() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("1.0.0.json");
    write_registry_json(&path, &record("sha256:one"), true)?;
    let published = fs::read_to_string(&path)?;

    let conflict = write_registry_json(&path, &record("sha256:two"), true);

    expect_io_failure(conflict, &path, io::ErrorKind::AlreadyExists)?;
    assert_eq!(fs::read_to_string(&path)?, published);
    // A failed publish leaves nothing behind for a reader to trip over.
    assert_eq!(
        entry_names(directory.path())?,
        vec!["1.0.0.json".to_owned()]
    );
    Ok(())
}

#[test]
fn registry_write_into_a_missing_directory_stages_nothing() -> TestResult {
    let directory = tempfile::tempdir()?;
    let absent = directory.path().join("acme").join("echo");
    let path = absent.join("1.0.0.json");

    let failure = write_registry_json(&path, &record("sha256:one"), false);

    expect_io_failure(failure, &path, io::ErrorKind::NotFound)?;
    assert!(!absent.exists());
    assert!(entry_names(directory.path())?.is_empty());
    Ok(())
}

/// Staging happens inside the skill directory that `list_versions` scans, so
/// the staged name must never look like a version record or a skill entry.
#[test]
fn staged_registry_names_are_not_readable_as_version_records() {
    let staged = format!("{STAGING_PREFIX}abc123{STAGING_SUFFIX}");
    assert!(!staged.ends_with(".json"));
    assert!(staged.starts_with('.'));
    assert!(!super::is_unsafe_path_component(&staged));
}

#[cfg(unix)]
#[test]
fn registry_update_preserves_a_read_only_record() -> TestResult {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir()?;
    let path = directory.path().join("1.0.0.json");
    write_registry_json(&path, &record("sha256:one"), true)?;
    let original = fs::read_to_string(&path)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o400))?;
    let result = write_registry_json(&path, &record("sha256:two"), false);
    // Restore fixture permissions before propagating an assertion failure.
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    expect_io_failure(result, &path, io::ErrorKind::PermissionDenied)?;
    assert_eq!(fs::read_to_string(&path)?, original);
    assert_eq!(
        entry_names(directory.path())?,
        vec!["1.0.0.json".to_owned()]
    );
    Ok(())
}
