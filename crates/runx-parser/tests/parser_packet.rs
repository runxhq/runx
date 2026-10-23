use std::collections::BTreeMap;

use runx_parser::{
    PacketSchemaError, SkillPackageSource, parse_packet_schema_document, validate_skill_package,
};

const PLAN: &str = r#"{"$id":"https://schemas.runx.ai/packages/acme/planner/plan/v1.json","x-runx-packet-id":"acme/planner.plan.v1","type":"object"}"#;

#[test]
fn packet_schema_parser_owns_identity_and_source_digest() -> Result<(), PacketSchemaError> {
    let schema = parse_packet_schema_document("plan.v1.schema.json", PLAN)?.ok_or_else(|| {
        PacketSchemaError::InvalidPacketId {
            path: "plan.v1.schema.json".to_owned(),
        }
    })?;

    assert_eq!(schema.packet_id, "acme/planner.plan.v1");
    assert!(schema.digest.starts_with("sha256:"));
    Ok(())
}

#[test]
fn json_without_packet_identity_is_not_a_packet_schema() -> Result<(), PacketSchemaError> {
    assert!(parse_packet_schema_document("plain.json", r#"{"type":"object"}"#)?.is_none());
    Ok(())
}

#[test]
fn present_packet_identity_must_be_a_packet_id() {
    for source in [
        r#"{"x-runx-packet-id":""}"#,
        r#"{"x-runx-packet-id":"   "}"#,
        r#"{"x-runx-packet-id":42}"#,
        r#"{"x-runx-packet-id":"planner.plan.v1"}"#,
    ] {
        assert!(matches!(
            parse_packet_schema_document("invalid.json", source),
            Err(PacketSchemaError::InvalidPacketId { .. })
        ));
    }
}

#[test]
fn a_packet_file_never_declares_a_core_id() {
    assert!(matches!(
        parse_packet_schema_document(
            "receipt.json",
            r#"{"$id":"https://schemas.runx.ai/runx/receipt/v1.json","x-runx-packet-id":"runx.receipt.v1"}"#,
        ),
        Err(PacketSchemaError::CorePacketFile { .. })
    ));
}

#[test]
fn a_packet_file_declares_its_canonical_schema_url() {
    for source in [
        r#"{"x-runx-packet-id":"acme/planner.plan.v1"}"#,
        r#"{"$id":"https://example.com/plan.json","x-runx-packet-id":"acme/planner.plan.v1"}"#,
    ] {
        assert!(matches!(
            parse_packet_schema_document("plan.v1.schema.json", source),
            Err(PacketSchemaError::SchemaUrl { .. })
        ));
    }
}

fn package(name: &str, packets: &[(&str, &str)]) -> SkillPackageSource {
    let mut files = BTreeMap::from([(
        "SKILL.md".to_owned(),
        format!("---\nname: {name}\ndescription: Plans.\n---\n\n# Planner\n").into_bytes(),
    )]);
    for (path, source) in packets {
        files.insert((*path).to_owned(), source.as_bytes().to_vec());
    }
    SkillPackageSource {
        files,
        ..SkillPackageSource::default()
    }
}

fn admission_error(source: SkillPackageSource) -> String {
    validate_skill_package(source)
        .err()
        .map(|error| error.to_string())
        .unwrap_or_default()
}

const LEAD: &str = r#"{"$id":"https://schemas.runx.ai/packages/acme/lead-qualify/qualification/v1.json","x-runx-packet-id":"acme/lead-qualify.qualification.v1","type":"object"}"#;

#[test]
fn a_package_carries_its_own_packets_and_pinned_copies_of_others()
-> Result<(), Box<dyn std::error::Error>> {
    let validated = validate_skill_package(package(
        "planner",
        &[
            ("packets/plan.v1.schema.json", PLAN),
            (
                "packets/acme/lead-qualify/qualification.v1.schema.json",
                LEAD,
            ),
        ],
    ))?;
    let ids = validated
        .packets
        .keys()
        .map(runx_contracts::PacketId::as_str)
        .collect::<Vec<_>>();
    assert_eq!(
        ids,
        ["acme/lead-qualify.qualification.v1", "acme/planner.plan.v1"]
    );
    assert!(
        validated
            .consumed_files
            .contains("packets/plan.v1.schema.json")
    );
    assert!(
        validated
            .consumed_files
            .contains("packets/acme/lead-qualify/qualification.v1.schema.json")
    );
    Ok(())
}

#[test]
fn a_package_never_defines_a_packet_under_another_package_name() {
    let error = admission_error(package(
        "planner",
        &[("packets/qualification.v1.schema.json", LEAD)],
    ));
    assert!(
        error.contains("belongs at packets/acme/lead-qualify/qualification.v1.schema.json"),
        "{error}"
    );
}

#[test]
fn an_own_packet_file_is_named_for_its_id() {
    let error = admission_error(package("planner", &[("packets/planner.json", PLAN)]));
    assert!(
        error.contains("belongs at packets/plan.v1.schema.json"),
        "{error}"
    );
}

#[test]
fn packets_holds_only_packet_schemas() {
    let error = admission_error(package(
        "planner",
        &[("packets/notes.json", r#"{"type":"object"}"#)],
    ));
    assert!(error.contains("holds only packet schemas"), "{error}");
}
