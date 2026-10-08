use runx_contracts::DoctorReport;

const FIXTURES: &[&str] = &[
    include_str!("../../../fixtures/doctor/cross-package-reach-in/expected.json"),
    include_str!("../../../fixtures/doctor/empty-success/expected.json"),
    include_str!("../../../fixtures/doctor/skill-fixture-missing/expected.json"),
    include_str!("../../../fixtures/doctor/skill-frontmatter-invalid/expected.json"),
    include_str!("../../../fixtures/doctor/tool-fixture-missing/expected.json"),
];

#[test]
fn doctor_fixtures_match_typescript_wire_shape() -> Result<(), serde_json::Error> {
    for fixture_json in FIXTURES {
        let expected: serde_json::Value = serde_json::from_str(fixture_json)?;
        let parsed: DoctorReport = serde_json::from_value(expected.clone())?;
        let actual = serde_json::to_value(parsed)?;
        assert_eq!(actual, expected);
    }
    Ok(())
}

#[test]
fn doctor_report_rejects_unknown_fixed_fields() {
    let value = serde_json::json!({
        "schema": "runx.doctor.v1",
        "status": "success",
        "summary": {
            "errors": 0,
            "warnings": 0,
            "infos": 0
        },
        "diagnostics": [],
        "unexpected": true
    });

    assert!(serde_json::from_value::<DoctorReport>(value).is_err());
}

#[test]
fn doctor_diagnostic_rejects_unknown_fixed_fields() {
    let value = serde_json::json!({
        "schema": "runx.doctor.v1",
        "status": "failure",
        "summary": {
            "errors": 1,
            "warnings": 0,
            "infos": 0
        },
        "diagnostics": [
            {
                "id": "runx.example",
                "instance_id": "sha256:example",
                "severity": "error",
                "title": "Example",
                "message": "Example",
                "target": {},
                "location": {
                    "path": "."
                },
                "repairs": [],
                "unexpected": true
            }
        ]
    });

    assert!(serde_json::from_value::<DoctorReport>(value).is_err());
}

#[test]
fn doctor_target_and_evidence_allow_flexible_objects() -> Result<(), serde_json::Error> {
    let value = serde_json::json!({
        "schema": "runx.doctor.v1",
        "status": "failure",
        "summary": {
            "errors": 1,
            "warnings": 0,
            "infos": 0
        },
        "diagnostics": [
            {
                "id": "runx.example",
                "instance_id": "sha256:example",
                "severity": "error",
                "title": "Example",
                "message": "Example",
                "target": {
                    "kind": "workspace",
                    "nested": {
                        "value": true
                    }
                },
                "location": {
                    "path": "."
                },
                "evidence": {
                    "count": 1,
                    "nested": {
                        "value": "ok"
                    }
                },
                "repairs": []
            }
        ]
    });

    let parsed: DoctorReport = serde_json::from_value(value)?;
    assert_eq!(parsed.diagnostics.len(), 1);
    Ok(())
}

#[test]
fn doctor_optional_fields_reject_null() {
    let value = serde_json::json!({
        "schema": "runx.doctor.v1",
        "status": "failure",
        "summary": {
            "errors": 1,
            "warnings": 0,
            "infos": 0
        },
        "diagnostics": [
            {
                "id": "runx.example",
                "instance_id": "sha256:example",
                "severity": "error",
                "title": "Example",
                "message": "Example",
                "target": {
                    "kind": "workspace"
                },
                "location": {
                    "path": "."
                },
                "evidence": null,
                "repairs": []
            }
        ]
    });

    assert!(serde_json::from_value::<DoctorReport>(value).is_err());
}
