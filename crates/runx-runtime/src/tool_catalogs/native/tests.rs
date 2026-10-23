#![allow(clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;

use std::collections::BTreeMap;

use runx_contracts::{JsonObject, JsonValue};

use super::{core_capabilities, definition, resends_safely};
use crate::credentials::CredentialDelivery;
use crate::effects::RuntimeEffectRegistry;

#[test]
fn capability_registry_definitions_are_unique_and_valid() {
    let capabilities = core_capabilities().collect::<Vec<_>>();
    let ids = capabilities
        .iter()
        .map(|capability| capability.definition().id)
        .collect::<BTreeSet<_>>();

    assert_eq!(ids.len(), capabilities.len());
    for capability in capabilities {
        crate::capability::validate_capability_contract(capability)
            .unwrap_or_else(|error| panic!("{}: {error}", capability.definition().id));
        let output_schema = capability.output_schema();
        assert!(output_schema.is_object());
        assert_eq!(
            definition(capability.definition().id)
                .expect("registered capability should resolve")
                .definition(),
            capability.definition()
        );
    }
}

#[test]
fn capability_snapshot_is_sorted_and_pins_the_catalog() {
    let snapshot = serde_json::to_value(super::native_capability_snapshot())
        .expect("snapshot should serialize");
    assert_eq!(
        snapshot.get("schema").and_then(serde_json::Value::as_str),
        Some("runx.native_capability_snapshot.v1")
    );
    let capabilities = snapshot
        .get("capabilities")
        .and_then(serde_json::Value::as_array)
        .expect("snapshot should contain capabilities");
    assert_eq!(capabilities.len(), super::core_capabilities().count());
    let ids = capabilities
        .iter()
        .filter_map(|capability| capability.get("id"))
        .filter_map(serde_json::Value::as_str)
        .collect::<Vec<_>>();
    assert!(ids.windows(2).all(|pair| pair[0] < pair[1]));
    for capability in capabilities {
        for field in [
            "id",
            "source",
            "scopes",
            "effect",
            "approval",
            "artifacts",
            "execution_boundary",
        ] {
            assert!(capability.get(field).is_some(), "missing {field}");
        }
        if let Some(packet) = capability
            .pointer("/artifacts/packet")
            .and_then(serde_json::Value::as_str)
        {
            assert!(
                packet.starts_with("runx."),
                "runtime-owned packet must use the runx namespace: {packet}"
            );
        }
    }
}

#[test]
fn capability_registry_defaults_keep_declared_json_types() {
    let capability = definition("evidence.index_sources")
        .expect("native evidence capability should be registered");
    let default = capability
        .catalog_inputs()
        .expect("catalog inputs should derive from the typed definition")
        .get("max_sources")
        .and_then(|input| input.default.as_ref())
        .cloned()
        .expect("max_sources should have a typed default");

    assert_eq!(
        serde_json::to_value(default).expect("serialize default"),
        20
    );
}

#[test]
fn capability_registry_owns_inspect_search_and_artifacts() {
    let effects =
        RuntimeEffectRegistry::with_effect(crate::kernel::operation::OperationEffect::default())
            .expect("the operation effect should register");
    let inspected = super::inspect("runx.skill.apply", std::path::Path::new("."), &effects)
        .expect("native capability should inspect");
    assert_eq!(inspected.tool.execution, "native");
    assert!(super::artifacts("runx.skill.apply", &effects).is_some());
    assert!(
        super::search("skill", 20, &effects)
            .iter()
            .any(|tool| tool.name == "runx.skill.apply")
    );
    assert_eq!(super::execution_boundary("acme.lookup", &effects), None);
}

#[test]
fn capability_registry_dispatch_projects_only_declared_inputs() {
    let inputs = JsonObject::from([
        ("value".to_owned(), JsonValue::String("hello".to_owned())),
        ("undeclared".to_owned(), JsonValue::Bool(true)),
    ]);
    let result = super::invoke(super::NativeToolInvocation {
        tool: "data.digest",
        observed_at: "2026-07-20T00:00:00Z",
        inputs,
        scopes: &[],
        env: &BTreeMap::new(),
        skill_directory: std::path::Path::new("."),
        credential_delivery: &CredentialDelivery::none(),
        local_artifacts: super::fixture_local_artifacts(),
        effect_admission: None,
        effects: &RuntimeEffectRegistry::default(),
        bounds: crate::services::WorkspaceBounds::Operator,
        call: None,
    })
    .expect("capability should resolve");

    assert_eq!(
        result.execution_boundary,
        runx_contracts::ExecutionBoundaryKind::NativeCapability
    );
    let output = result
        .result
        .expect("ambient graph inputs must not enter the typed handler");
    assert!(
        output
            .as_object()
            .is_some_and(|output| output.contains_key("digest_result"))
    );
}

#[test]
fn capability_registry_exposes_one_input_schema() {
    let capability = definition("fs.read").expect("fs.read should be registered");
    let schema = capability
        .input_schema()
        .expect("typed schema should derive");
    let catalog = capability.catalog_inputs().expect("catalog should derive");
    let properties = schema
        .get("properties")
        .and_then(serde_json::Value::as_object)
        .expect("input schema should expose properties");

    assert_eq!(
        properties.keys().cloned().collect::<BTreeSet<_>>(),
        catalog.keys().cloned().collect::<BTreeSet<_>>()
    );
}

#[test]
fn every_capability_offers_the_input_shape_it_decodes() {
    for capability in core_capabilities() {
        let id = capability.definition().id;
        let typed = capability
            .input_schema()
            .expect("typed schema should derive");
        let catalog = capability.catalog_inputs().expect("catalog should derive");
        let offered = serde_json::to_value(runx_contracts::input_contract_schema(&catalog))
            .expect("offered schema should serialize");
        for (name, property) in typed
            .get("properties")
            .and_then(serde_json::Value::as_object)
            .expect("typed schema should expose properties")
        {
            // An optional property's value, when present, takes its choice other than null.
            let present = property
                .get("anyOf")
                .and_then(serde_json::Value::as_array)
                .and_then(|choices| {
                    choices.iter().find(|choice| {
                        choice.get("type").and_then(serde_json::Value::as_str) != Some("null")
                    })
                })
                .unwrap_or(property);
            let offered = &offered["properties"][name];
            for (keyword, value) in present.as_object().expect("a property is an object") {
                assert_eq!(&offered[keyword], value, "{id} {name} offers its {keyword}");
            }
        }
    }
}

#[test]
fn a_capability_input_offers_the_defaults_decoding_takes_written_out() {
    let offered = |id: &str| {
        let catalog = definition(id)
            .expect("capability should be registered")
            .catalog_inputs()
            .expect("catalog should derive");
        serde_json::to_value(runx_contracts::input_contract_schema(&catalog))
            .expect("offered schema should serialize")
    };
    let command = offered("command.execute");
    // The result carries the command's output unless the call asks otherwise.
    assert_eq!(
        command["properties"]["output_mode"]["default"],
        serde_json::json!("text")
    );
    assert_eq!(
        command["properties"]["args"],
        serde_json::json!({
            "type": "array",
            "items": { "type": "string" },
            "description": "Zero to 128 exact string arguments.",
        })
    );
    let edit = offered("fs.edit");
    assert_eq!(
        edit["properties"]["edits"]["items"]["properties"]["replace_all"],
        serde_json::json!({ "type": "boolean" })
    );
}

#[test]
fn native_output_contract_is_exact_and_rejects_invalid_inner_fields() {
    let capability = definition("data.digest").expect("data.digest should be registered");
    let schema = capability.output_schema();
    let digest_result = schema
        .get("properties")
        .and_then(serde_json::Value::as_object)
        .and_then(|properties| properties.get("digest_result"))
        .and_then(serde_json::Value::as_object)
        .expect("digest_result should have a typed schema");
    let algorithm = digest_result
        .get("properties")
        .and_then(serde_json::Value::as_object)
        .and_then(|properties| properties.get("algorithm"))
        .expect("algorithm should be declared");
    assert_eq!(
        algorithm.get("type").and_then(serde_json::Value::as_str),
        Some("string")
    );

    let invalid = JsonValue::Object(JsonObject::from([(
        "digest_result".to_owned(),
        JsonValue::Object(JsonObject::from([
            (
                "algorithm".to_owned(),
                JsonValue::Number(runx_contracts::JsonNumber::U64(1)),
            ),
            (
                "digest".to_owned(),
                JsonValue::String(
                    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                        .to_owned(),
                ),
            ),
        ])),
    )]));
    assert!(capability.validate_output(&invalid).is_err());
}

#[test]
fn a_read_or_a_journaled_change_can_run_again_and_a_process_cannot() {
    for safe in [
        "fs.read",
        "fs.search",
        "git.log",
        "data.digest",
        "fs.write",
        "fs.edit",
    ] {
        assert!(resends_safely(safe), "{safe} can run again unseen");
    }
    for unsafe_tool in ["command.execute", "an.unknown.tool"] {
        assert!(
            !resends_safely(unsafe_tool),
            "{unsafe_tool} is not resent unseen"
        );
    }
}

#[test]
fn every_tool_that_changes_something_describes_its_effect() {
    for capability in core_capabilities() {
        let definition = capability.definition();
        assert_eq!(
            capability.describes_effect(),
            definition.effect.mutating(),
            "{} describes its effect exactly when it changes something",
            definition.id
        );
    }
}
