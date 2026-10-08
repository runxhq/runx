use std::fs;
use std::path::PathBuf;

use runx_runtime::{DoctorOptions, run_doctor};

const DOCTOR_FIXTURES: &[&str] = &[
    "cross-package-reach-in",
    "empty-success",
    "skill-fixture-missing",
    "skill-frontmatter-invalid",
    "tool-fixture-missing",
];

#[test]
fn doctor_runtime_matches_all_fixture_reports() -> Result<(), Box<dyn std::error::Error>> {
    let fixture_root = fixture_root();
    for fixture_name in DOCTOR_FIXTURES {
        let case_root = fixture_root.join(fixture_name);
        let expected_json = fs::read_to_string(case_root.join("expected.json"))?;
        let expected: serde_json::Value = serde_json::from_str(&expected_json)?;

        let report = run_doctor(&case_root.join("workspace"), &DoctorOptions)?;
        let actual = serde_json::to_value(report)?;

        if std::env::var_os("RUNX_UPDATE_DOCTOR_FIXTURES").is_some() {
            update_fixture_instance_ids(&case_root.join("expected.json"), &expected, &actual)?;
            continue;
        }

        assert_eq!(actual, expected, "doctor fixture {fixture_name}");
    }
    Ok(())
}

/// Rewrite only the `instance_id` values in a committed fixture, preserving its
/// TypeScript wire key order and pretty formatting. Diagnostics are positionally
/// aligned: the report and fixture differ only in the regenerated hashes.
fn update_fixture_instance_ids(
    fixture_path: &std::path::Path,
    expected: &serde_json::Value,
    actual: &serde_json::Value,
) -> Result<(), Box<dyn std::error::Error>> {
    let expected_ids = diagnostic_instance_ids(expected);
    let actual_ids = diagnostic_instance_ids(actual);
    assert_eq!(
        expected_ids.len(),
        actual_ids.len(),
        "diagnostic count drift for {}",
        fixture_path.display()
    );

    let mut contents = fs::read_to_string(fixture_path)?;
    for (old_id, new_id) in expected_ids.iter().zip(actual_ids.iter()) {
        if old_id != new_id {
            contents = contents.replace(old_id, new_id);
        }
    }
    fs::write(fixture_path, contents)?;
    Ok(())
}

fn diagnostic_instance_ids(report: &serde_json::Value) -> Vec<String> {
    report
        .get("diagnostics")
        .and_then(serde_json::Value::as_array)
        .map(|diagnostics| {
            diagnostics
                .iter()
                .filter_map(|diagnostic| {
                    diagnostic
                        .get("instance_id")
                        .and_then(serde_json::Value::as_str)
                        .map(ToOwned::to_owned)
                })
                .collect()
        })
        .unwrap_or_default()
}

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("fixtures")
        .join("doctor")
}
