use serde_json::{Value, json};

pub(super) fn act_result_corpus() -> Vec<(&'static str, Value)> {
    let terminal = json!({
        "status": "sealed",
        "stdout": "ok",
        "stderr": "",
        "exitCode": 0,
        "signal": null,
        "durationMs": 12,
    });
    let needs_agent = json!({
        "status": "needs_agent",
        "stdout": "",
        "stderr": "",
        "exitCode": null,
        "signal": null,
        "durationMs": 0,
        "request": {
            "id": "req_1",
            "kind": "input",
            "questions": [],
        },
    });
    vec![
        ("terminal valid", terminal.clone()),
        ("terminal full valid", {
            let mut v = terminal.clone();
            v["status"] = json!("failure");
            v["exitCode"] = json!(null);
            v["signal"] = json!("SIGTERM");
            v["errorMessage"] = json!("failed");
            v["metadata"] = json!({ "k": true });
            v
        }),
        ("needs_agent valid", needs_agent.clone()),
        (
            "terminal missing exitCode",
            drop_field(terminal.clone(), "exitCode"),
        ),
        (
            "terminal unknown status",
            set_field(terminal.clone(), "status", json!("done")),
        ),
        (
            "terminal unknown signal",
            set_field(terminal.clone(), "signal", json!("SIGNOPE")),
        ),
        (
            "needs_agent non-null exitCode",
            set_field(needs_agent.clone(), "exitCode", json!(1)),
        ),
        (
            "needs_agent missing request",
            drop_field(needs_agent.clone(), "request"),
        ),
        (
            "needs_agent unknown request kind",
            set_field(
                needs_agent.clone(),
                "request",
                json!({ "id": "req_1", "kind": "other" }),
            ),
        ),
        (
            "additional property",
            set_field(terminal.clone(), "bogus", json!(true)),
        ),
    ]
}

pub(super) fn dev_report_corpus() -> Vec<(&'static str, Value)> {
    let valid = json!({
        "schema": "runx.dev.v1",
        "status": "success",
        "doctor": doctor_valid_report(),
        "fixtures": [],
    });
    vec![
        ("minimal valid", valid.clone()),
        ("full valid", {
            let mut v = valid.clone();
            v["fixtures"] = json!([{
                "name": "fixture-1",
                "lane": "deterministic",
                "target": { "kind": "tool" },
                "status": "success",
                "duration_ms": 12,
                "assertions": [{
                    "path": "$.status",
                    "expected": "sealed",
                    "actual": "sealed",
                    "kind": "exact_mismatch",
                    "message": "checked",
                }],
                "skip_reason": "none",
                "output": { "ok": true },
                "replay_path": "fixtures/a.yaml",
            }]);
            v["receipt_id"] = json!("rcpt_1");
            v
        }),
        ("missing schema", drop_field(valid.clone(), "schema")),
        (
            "wrong schema",
            set_field(valid.clone(), "schema", json!("runx.old")),
        ),
        ("missing doctor", drop_field(valid.clone(), "doctor")),
        (
            "unknown status",
            set_field(valid.clone(), "status", json!("done")),
        ),
        (
            "fixture unknown status",
            set_field(
                valid.clone(),
                "fixtures",
                json!([{
                    "name": "fixture-1",
                    "lane": "deterministic",
                    "target": {},
                    "status": "done",
                    "duration_ms": 1,
                    "assertions": [],
                }]),
            ),
        ),
        (
            "fixture assertion unknown kind",
            set_field(
                valid.clone(),
                "fixtures",
                json!([{
                    "name": "fixture-1",
                    "lane": "deterministic",
                    "target": {},
                    "status": "success",
                    "duration_ms": 1,
                    "assertions": [{
                        "path": "$",
                        "kind": "surprise",
                        "message": "bad",
                    }],
                }]),
            ),
        ),
        (
            "additional property",
            set_field(valid.clone(), "bogus", json!(true)),
        ),
    ]
}

pub(super) fn fixture_corpus() -> Vec<(&'static str, Value)> {
    let valid = json!({
        "name": "fixture-1",
        "lane": "deterministic",
        "target": { "kind": "tool", "tool": "echo" },
        "expect": { "status": "sealed" },
    });
    vec![
        ("minimal valid", valid.clone()),
        ("full valid", {
            let mut v = valid.clone();
            v["inputs"] = json!({ "message": "hi" });
            v["env"] = json!({ "RUNX": "1" });
            v["agent"] = json!({ "model": "fixture" });
            v["repo"] = json!({ "path": "." });
            v["execution"] = json!({ "timeout_ms": 10 });
            v["permissions"] = json!({ "network": false });
            v
        }),
        ("missing name", drop_field(valid.clone(), "name")),
        ("missing expect", drop_field(valid.clone(), "expect")),
        (
            "unknown lane",
            set_field(valid.clone(), "lane", json!("manual")),
        ),
        (
            "additional property",
            set_field(valid.clone(), "bogus", json!(true)),
        ),
    ]
}

pub(super) fn tool_manifest_corpus() -> Vec<(&'static str, Value)> {
    let valid = json!({
        "schema": "runx.tool.manifest.v1",
        "name": "fs.read",
        "description": "Read a file",
        "source": {
            "command": "node",
            "args": ["tool.mjs"],
            "input_mode": "stdin",
            "environment": {
                "optional": ["HOME"]
            }
        },
        "inputs": {
            "path": { "type": "string", "required": true }
        },
        "scopes": ["fs.read"],
        "risk": { "level": "low" },
        "runx": { "owner": "runx" },
        "runtime": {
            "command": "node",
            "args": ["tool.mjs"],
            "env": {}
        },
        "output": {
            "packet": "file",
            "description": "Read file contents"
        },
        "retry": { "max_attempts": 2 },
        "idempotency": { "key": "path" },
        "source_hash": "sha256:source",
        "schema_hash": "sha256:schema"
    });
    vec![
        ("valid tool manifest", valid.clone()),
        (
            "valid with version",
            set_field(valid.clone(), "version", json!("1.2.3")),
        ),
        ("missing schema", drop_field(valid.clone(), "schema")),
        ("missing source", drop_field(valid.clone(), "source")),
        (
            "unknown source type",
            set_field(
                valid.clone(),
                "source",
                set_field(valid["source"].clone(), "type", json!("unknown")),
            ),
        ),
        (
            "bad input mode",
            set_field(
                valid.clone(),
                "source",
                set_field(valid["source"].clone(), "input_mode", json!("stream")),
            ),
        ),
        (
            "missing runtime command",
            set_field(
                valid.clone(),
                "runtime",
                drop_field(valid["runtime"].clone(), "command"),
            ),
        ),
        (
            "additional property",
            set_field(valid.clone(), "bogus", json!(true)),
        ),
    ]
}

pub(super) fn list_corpus() -> Vec<(&'static str, Value)> {
    let valid = json!({
        "schema": "runx.list.v1",
        "root": "/tmp/runx",
        "requested_kind": "all",
        "items": [],
    });
    vec![
        ("minimal valid", valid.clone()),
        ("full valid", {
            let mut v = valid.clone();
            v["items"] = json!([{
                "kind": "tool",
                "name": "echo",
                "source": "local",
                "path": "tools/demo/echo/manifest.json",
                "status": "ok",
                "diagnostics": [],
                "scopes": ["repo:read"],
                "emits": [{ "name": "report", "packet": "runx.report.v1" }],
                "fixtures": 2,
                "harness_cases": 1,
                "steps": 0,
                "wraps": "value",
            }]);
            v
        }),
        ("missing schema", drop_field(valid.clone(), "schema")),
        (
            "wrong schema",
            set_field(valid.clone(), "schema", json!("runx.old")),
        ),
        (
            "unknown requested_kind",
            set_field(valid.clone(), "requested_kind", json!("everything")),
        ),
        (
            "item unknown source",
            set_field(
                valid.clone(),
                "items",
                json!([{
                    "kind": "tool",
                    "name": "echo",
                    "source": "remote",
                    "path": "tools/demo/echo/manifest.json",
                    "status": "ok",
                }]),
            ),
        ),
        (
            "additional property",
            set_field(valid.clone(), "bogus", json!(true)),
        ),
    ]
}

pub(super) fn run_summary_corpus() -> Vec<(&'static str, Value)> {
    let valid = json!({
        "schema": "runx.run-summary.v1",
        "run_id": "rx_1",
        "command": "runx harness",
        "status": "success",
        "started_at": "2026-01-01T00:00:00Z",
        "root": "/tmp/runx",
        "steps": [],
    });
    vec![
        ("minimal valid", valid.clone()),
        ("full valid", {
            let mut v = valid.clone();
            v["finished_at"] = json!("2026-01-01T00:00:01Z");
            v["unit"] = json!({ "skill": "demo" });
            v["steps"] = json!([{ "id": "step_1", "status": "success" }]);
            v["receipt_ref"] = json!("runx:receipt:rcpt_1");
            v
        }),
        ("missing schema", drop_field(valid.clone(), "schema")),
        (
            "wrong schema",
            set_field(valid.clone(), "schema", json!("runx.old")),
        ),
        ("missing run_id", drop_field(valid.clone(), "run_id")),
        (
            "unknown status",
            set_field(valid.clone(), "status", json!("done")),
        ),
        (
            "additional property",
            set_field(valid.clone(), "bogus", json!(true)),
        ),
    ]
}

pub(super) fn ledger_entry_corpus() -> Vec<(&'static str, Value)> {
    let hash = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let valid = json!({
        "schema_version": "runx.ledger.entry.v1",
        "chain": {
            "version": "runx.ledger.chain.v1",
            "algorithm": "sha256",
            "canonicalization": "runx.stable-json.v1",
            "index": 0,
            "previous_hash": null,
            "entry_hash": hash,
        },
        "entry": {
            "type": "run_event",
            "version": "1",
            "data": { "kind": "started" },
            "meta": {
                "artifact_id": "artifact_1",
                "run_id": "rx_1",
                "step_id": null,
                "producer": { "skill": "demo", "runner": "local" },
                "created_at": "2026-01-01T00:00:00Z",
                "hash": "sha256:abc",
                "size_bytes": 1,
                "parent_artifact_id": null,
                "receipt_id": null,
                "redacted": false,
            },
        },
    });
    vec![
        ("minimal valid", valid.clone()),
        ("full valid", {
            let mut v = valid.clone();
            v["chain"]["index"] = json!(1);
            v["chain"]["previous_hash"] = json!(hash);
            v["entry"]["meta"]["step_id"] = json!("step_1");
            v["entry"]["meta"]["parent_artifact_id"] = json!("artifact_0");
            v["entry"]["meta"]["receipt_id"] = json!("rcpt_1");
            v
        }),
        (
            "missing schema_version",
            drop_field(valid.clone(), "schema_version"),
        ),
        (
            "wrong schema_version",
            set_field(valid.clone(), "schema_version", json!("runx.old")),
        ),
        (
            "wrong chain version",
            set_field(
                valid.clone(),
                "chain",
                set_field(valid["chain"].clone(), "version", json!("runx.old")),
            ),
        ),
        (
            "invalid hash pattern",
            set_field(
                valid.clone(),
                "chain",
                set_field(valid["chain"].clone(), "entry_hash", json!("not-a-hash")),
            ),
        ),
        (
            "empty artifact_id",
            set_field(
                valid.clone(),
                "entry",
                set_field(
                    valid["entry"].clone(),
                    "meta",
                    set_field(valid["entry"]["meta"].clone(), "artifact_id", json!("")),
                ),
            ),
        ),
        (
            "additional property",
            set_field(valid.clone(), "bogus", json!(true)),
        ),
    ]
}

fn doctor_valid_report() -> Value {
    json!({
        "schema": "runx.doctor.v1",
        "status": "success",
        "summary": { "errors": 0, "warnings": 0, "infos": 0 },
        "diagnostics": [],
    })
}

pub(super) fn scope_admission_corpus() -> Vec<(&'static str, Value)> {
    // `decision_summary` is optional on the committed wire contract. Keep a
    // no-summary valid case so the Rust source stays aligned with that shape.
    let valid = json!({
        "status": "allow",
        "requested_scopes": ["issues:write"],
        "granted_scopes": ["issues:write"],
    });
    vec![
        ("minimal valid", valid.clone()),
        ("full valid", {
            let mut v = valid.clone();
            v["grant_id"] = json!("grant_1");
            v["reasons"] = json!(["policy allowed"]);
            v["decision_summary"] = json!("granted");
            v
        }),
        ("missing status", drop_field(valid.clone(), "status")),
        (
            "missing requested_scopes",
            drop_field(valid.clone(), "requested_scopes"),
        ),
        (
            "empty requested scope",
            set_field(valid.clone(), "requested_scopes", json!([""])),
        ),
        (
            "empty grant_id",
            set_field(valid.clone(), "grant_id", json!("")),
        ),
        (
            "empty reason",
            set_field(valid.clone(), "reasons", json!([""])),
        ),
        (
            "unknown status",
            set_field(valid.clone(), "status", json!("maybe")),
        ),
        (
            "status as object",
            set_field(valid.clone(), "status", json!({})),
        ),
        (
            "additional property",
            set_field(valid.clone(), "bogus", json!(true)),
        ),
    ]
}

pub(super) fn credential_envelope_corpus() -> Vec<(&'static str, Value)> {
    let valid = json!({
        "kind": "runx.credential-envelope.v1",
        "grant_id": "grant_1",
        "provider": "github",
        "auth_mode": "api_key",
        "material_kind": "token",
        "provider_reference": "provider-ref-1",
        "scopes": ["issues:write"],
        "material_ref": "ref:abc",
    });
    vec![
        ("minimal valid", valid.clone()),
        ("full valid (grant_reference)", {
            let mut v = valid.clone();
            v["grant_reference"] = json!({
                "grant_id": "grant_1",
                "scope_family": "github",
                "authority_kind": "constructive",
                "target_repo": "acme/widgets",
                "target_locator": "issue/1",
            });
            v
        }),
        ("missing grant_id", drop_field(valid.clone(), "grant_id")),
        ("missing provider", drop_field(valid.clone(), "provider")),
        (
            "missing provider_reference",
            drop_field(valid.clone(), "provider_reference"),
        ),
        (
            "missing material_ref",
            drop_field(valid.clone(), "material_ref"),
        ),
        (
            "wrong kind",
            set_field(valid.clone(), "kind", json!("runx.old")),
        ),
        (
            "empty grant_id",
            set_field(valid.clone(), "grant_id", json!("")),
        ),
        (
            "empty scope item",
            set_field(valid.clone(), "scopes", json!([""])),
        ),
        (
            "grant_reference unknown authority_kind",
            set_field(
                valid.clone(),
                "grant_reference",
                json!({
                    "grant_id": "g",
                    "scope_family": "github",
                    "authority_kind": "godmode",
                }),
            ),
        ),
        (
            "grant_reference missing grant_id",
            set_field(
                valid.clone(),
                "grant_reference",
                json!({ "scope_family": "github" }),
            ),
        ),
        (
            "grant_reference missing scope_family",
            set_field(
                valid.clone(),
                "grant_reference",
                json!({ "grant_id": "g", "authority_kind": "constructive" }),
            ),
        ),
        (
            "grant_reference missing authority_kind",
            set_field(
                valid.clone(),
                "grant_reference",
                json!({ "grant_id": "g", "scope_family": "github" }),
            ),
        ),
        (
            "additional property",
            set_field(valid.clone(), "bogus", json!(true)),
        ),
    ]
}

fn authority_proof_requested() -> Value {
    json!({
        "connected_auth": true,
        "scopes": ["issues:write"],
    })
}

fn authority_proof_credential_material() -> Value {
    json!({ "status": "resolved" })
}

fn authority_proof_redaction() -> Value {
    json!({
        "status": "applied",
        "secret_material": "omitted",
        "stdout": "hashed",
        "stderr": "hashed",
        "metadata_secret_keys": [],
    })
}

pub(super) fn authority_proof_corpus() -> Vec<(&'static str, Value)> {
    let valid = json!({
        "schema_version": "runx.authority-proof.v1",
        "skill_name": "demo",
        "source_type": "github_issue",
        "requested": authority_proof_requested(),
        "scope_admission": {
            "status": "allow",
            "requested_scopes": ["issues:write"],
            "granted_scopes": ["issues:write"],
            "decision_summary": "granted",
        },
        "credential_material": authority_proof_credential_material(),
        "execution_boundary": { "kind": "remote_provider" },
        "redaction": authority_proof_redaction(),
    });
    vec![
        ("minimal valid", valid.clone()),
        ("full valid (approval + targeting)", {
            let mut v = valid.clone();
            v["run_id"] = json!("run_1");
            v["requested"] = json!({
                "connected_auth": true,
                "scopes": ["issues:write"],
                "scope_family": "github",
                "authority_kind": "constructive",
                "target_repo": "acme/widgets",
                "target_locator": "issue/1",
            });
            v["credential_material"] = json!({
                "status": "resolved",
                "grant_id": "grant_1",
                "provider": "github",
                "provider_reference": "provider-ref-1",
                "scopes": ["issues:write"],
                "authority_kind": "constructive",
                "grant_reference": {
                    "grant_id": "grant_1",
                    "scope_family": "github",
                    "authority_kind": "constructive",
                },
            });
            v["approval_gate"] = json!({
                "gate_id": "gate_1",
                "gate_type": "human",
                "decision": "approved",
            });
            v
        }),
        (
            "missing schema_version",
            drop_field(valid.clone(), "schema_version"),
        ),
        (
            "wrong schema_version",
            set_field(valid.clone(), "schema_version", json!("runx.old")),
        ),
        (
            "empty skill_name",
            set_field(valid.clone(), "skill_name", json!("")),
        ),
        (
            "empty source_type",
            set_field(valid.clone(), "source_type", json!("")),
        ),
        ("missing requested", drop_field(valid.clone(), "requested")),
        (
            "missing scope_admission",
            drop_field(valid.clone(), "scope_admission"),
        ),
        ("missing redaction", drop_field(valid.clone(), "redaction")),
        (
            "requested unknown authority_kind",
            set_field(
                valid.clone(),
                "requested",
                set_field(
                    authority_proof_requested(),
                    "authority_kind",
                    json!("godmode"),
                ),
            ),
        ),
        (
            "scope_admission unknown status",
            set_field(
                valid.clone(),
                "scope_admission",
                json!({
                    "status": "maybe",
                    "requested_scopes": [],
                    "granted_scopes": [],
                    "decision_summary": "x",
                }),
            ),
        ),
        (
            "credential_material unknown status",
            set_field(
                valid.clone(),
                "credential_material",
                json!({ "status": "materialized" }),
            ),
        ),
        (
            "redaction wrong status",
            set_field(
                valid.clone(),
                "redaction",
                set_field(authority_proof_redaction(), "status", json!("pending")),
            ),
        ),
        (
            "redaction wrong secret posture",
            set_field(
                valid.clone(),
                "redaction",
                set_field(
                    authority_proof_redaction(),
                    "secret_material",
                    json!("inline"),
                ),
            ),
        ),
        (
            "redaction wrong stdout posture",
            set_field(
                valid.clone(),
                "redaction",
                set_field(authority_proof_redaction(), "stdout", json!("raw")),
            ),
        ),
        (
            "additional property",
            set_field(valid.clone(), "bogus", json!(true)),
        ),
    ]
}

pub(super) fn output_corpus() -> Vec<(&'static str, Value)> {
    // The per-value `OutputFieldSpec` commits `minProperties: 1` (a numeric bound
    // the emitter does not model); corpus spec values stay non-empty so both
    // validators agree.
    vec![
        ("empty map", json!({})),
        (
            "bare type values",
            json!({ "result": "string", "count": "integer" }),
        ),
        (
            "typed spec value",
            json!({ "report": { "type": "object", "required": true } }),
        ),
        (
            "spec with enum + wrap_as",
            json!({ "status": { "type": "string", "enum": ["ok", "fail"], "wrap_as": "value" } }),
        ),
        ("unknown bare type rejected", json!({ "result": "blob" })),
        (
            "spec unknown type rejected",
            json!({ "report": { "type": "blob" } }),
        ),
        (
            "spec additional property rejected",
            json!({ "report": { "type": "object", "bogus": true } }),
        ),
        ("not an object", json!("nope")),
    ]
}

pub(super) fn handoff_signal_corpus() -> Vec<(&'static str, Value)> {
    let valid = json!({
        "schema": "runx.handoff_signal.v1",
        "signal_id": "sig_1",
        "handoff_id": "ho_1",
        "source": "issue_comment",
        "disposition": "acknowledged",
        "recorded_at": "2026-01-01T00:00:00Z",
    });
    vec![
        ("minimal valid", valid.clone()),
        ("full valid", {
            let mut v = valid.clone();
            v["boundary_kind"] = json!("pull_request");
            v["target_repo"] = json!("acme/widgets");
            v["thread_locator"] = json!("acme/widgets#1");
            v["actor"] = json!({ "actor_id": "u1", "display_name": "User", "role": "maintainer" });
            v["notes"] = json!("looks good");
            v["labels"] = json!(["bug"]);
            v["source_ref"] = json!({ "type": "pull_request", "uri": "runx:pr:1", "label": "PR" });
            v["metadata"] = json!({ "k": 1 });
            v
        }),
        ("missing schema", drop_field(valid.clone(), "schema")),
        ("missing signal_id", drop_field(valid.clone(), "signal_id")),
        (
            "missing disposition",
            drop_field(valid.clone(), "disposition"),
        ),
        (
            "empty signal_id",
            set_field(valid.clone(), "signal_id", json!("")),
        ),
        (
            "unknown source",
            set_field(valid.clone(), "source", json!("smoke_signal")),
        ),
        (
            "unknown disposition",
            set_field(valid.clone(), "disposition", json!("ghosted")),
        ),
        (
            "malformed recorded_at",
            set_field(valid.clone(), "recorded_at", json!("nope")),
        ),
        (
            "source_ref empty uri rejected",
            set_field(
                valid.clone(),
                "source_ref",
                json!({ "type": "pull_request", "uri": "" }),
            ),
        ),
        (
            "additional property",
            set_field(valid.clone(), "bogus", json!(true)),
        ),
    ]
}

pub(super) fn handoff_state_corpus() -> Vec<(&'static str, Value)> {
    let valid = json!({
        "schema": "runx.handoff_state.v1",
        "handoff_id": "ho_1",
        "status": "awaiting_response",
        "signal_count": 0,
    });
    vec![
        ("minimal valid", valid.clone()),
        ("full valid", {
            let mut v = valid.clone();
            v["boundary_kind"] = json!("pull_request");
            v["target_repo"] = json!("acme/widgets");
            v["last_signal_id"] = json!("sig_9");
            v["last_signal_at"] = json!("2026-01-02T00:00:00Z");
            v["last_signal_disposition"] = json!("merged");
            v["suppression_record_id"] = json!("sup_1");
            v["suppression_reason"] = json!("operator_block");
            v["summary"] = json!("ongoing");
            v
        }),
        ("missing schema", drop_field(valid.clone(), "schema")),
        (
            "missing handoff_id",
            drop_field(valid.clone(), "handoff_id"),
        ),
        (
            "missing signal_count",
            drop_field(valid.clone(), "signal_count"),
        ),
        (
            "empty handoff_id",
            set_field(valid.clone(), "handoff_id", json!("")),
        ),
        (
            "unknown status",
            set_field(valid.clone(), "status", json!("ghosted")),
        ),
        // `signal_count` commits a `minimum: 0` bound the type-driven emitter
        // does not model (same gap as `minItems`/`minProperties`); keep the
        // corpus on the non-negative side so both validators agree.
        (
            "signal_count as string",
            set_field(valid.clone(), "signal_count", json!("two")),
        ),
        (
            "unknown suppression_reason",
            set_field(valid.clone(), "suppression_reason", json!("mood")),
        ),
        (
            "malformed last_signal_at",
            set_field(valid.clone(), "last_signal_at", json!("nope")),
        ),
        (
            "additional property",
            set_field(valid.clone(), "bogus", json!(true)),
        ),
    ]
}

pub(super) fn suppression_record_corpus() -> Vec<(&'static str, Value)> {
    let valid = json!({
        "schema": "runx.suppression_record.v1",
        "record_id": "sup_1",
        "scope": "contact",
        "key": "user@example.com",
        "reason": "requested_no_contact",
        "recorded_at": "2026-01-01T00:00:00Z",
    });
    vec![
        ("minimal valid", valid.clone()),
        ("full valid", {
            let mut v = valid.clone();
            v["expires_at"] = json!("2026-02-01T00:00:00Z");
            v["source_signal_id"] = json!("sig_1");
            v["notes"] = json!("per request");
            v
        }),
        ("missing schema", drop_field(valid.clone(), "schema")),
        ("missing record_id", drop_field(valid.clone(), "record_id")),
        ("missing reason", drop_field(valid.clone(), "reason")),
        (
            "empty record_id",
            set_field(valid.clone(), "record_id", json!("")),
        ),
        ("empty key", set_field(valid.clone(), "key", json!(""))),
        (
            "unknown scope",
            set_field(valid.clone(), "scope", json!("galaxy")),
        ),
        (
            "unknown reason",
            set_field(valid.clone(), "reason", json!("mood")),
        ),
        (
            "malformed expires_at",
            set_field(valid.clone(), "expires_at", json!("nope")),
        ),
        (
            "additional property",
            set_field(valid.clone(), "bogus", json!(true)),
        ),
    ]
}

pub(super) fn packet_index_corpus() -> Vec<(&'static str, Value)> {
    let valid = json!({
        "schema": "runx.packet.index.v1",
        "packets": [{
            "id": "p1",
            "package": "@runxhq/skill",
            "version": "1.0.0",
            "path": "skills/p1",
            "sha256": "abc",
        }],
    });
    vec![
        ("valid with packet", valid.clone()),
        (
            "valid empty packets",
            set_field(valid.clone(), "packets", json!([])),
        ),
        ("missing schema", drop_field(valid.clone(), "schema")),
        ("missing packets", drop_field(valid.clone(), "packets")),
        (
            "wrong schema const",
            set_field(valid.clone(), "schema", json!("runx.other.v1")),
        ),
        (
            "packet missing id",
            set_field(
                valid.clone(),
                "packets",
                json!([{ "package": "p", "version": "1", "path": "x", "sha256": "y" }]),
            ),
        ),
        (
            "packet additional property",
            set_field(
                valid.clone(),
                "packets",
                json!([{ "id": "p1", "package": "p", "version": "1", "path": "x", "sha256": "y", "bogus": true }]),
            ),
        ),
        (
            "additional property",
            set_field(valid.clone(), "bogus", json!(true)),
        ),
    ]
}

pub(super) fn registry_binding_corpus() -> Vec<(&'static str, Value)> {
    let valid = json!({
        "schema": "runx.registry_binding.v1",
        "state": "registry_bound",
        "skill": { "id": "s1", "name": "Skill", "description": "does a thing" },
        "upstream": {
            "host": "github.com",
            "owner": "acme",
            "repo": "skills",
            "path": "s1/SKILL.md",
            "commit": "deadbeef",
            "blob_sha": "cafef00d",
            "source_of_truth": true,
        },
        "registry": {
            "owner": "acme",
            "trust_tier": "first_party",
            "version": "1.0.0",
            "profile_path": "profiles/s1",
            "materialized_package_is_registry_artifact": true,
        },
        "harness": { "status": "harness_verified", "case_count": 3 },
    });
    vec![
        ("minimal valid", valid.clone()),
        ("full valid (open extras + optionals)", {
            let mut v = valid.clone();
            v["extra_top"] = json!("ok");
            v["upstream"]["branch"] = json!("main");
            v["upstream"]["pr_url"] = json!("https://github.com/acme/skills/pull/1");
            v["registry"]["install_command"] = json!("runx install s1");
            v["harness"]["assertion_count"] = json!(9);
            v["harness"]["case_names"] = json!(["c1", "c2"]);
            v
        }),
        ("missing schema", drop_field(valid.clone(), "schema")),
        ("missing state", drop_field(valid.clone(), "state")),
        ("missing harness", drop_field(valid.clone(), "harness")),
        (
            "wrong schema const",
            set_field(valid.clone(), "schema", json!("runx.other.v1")),
        ),
        (
            "unknown state",
            set_field(valid.clone(), "state", json!("limbo")),
        ),
        (
            "unknown trust_tier",
            set_field(
                valid.clone(),
                "registry",
                json!({
                    "owner": "acme",
                    "trust_tier": "platinum",
                    "version": "1.0.0",
                    "profile_path": "profiles/s1",
                    "materialized_package_is_registry_artifact": true,
                }),
            ),
        ),
        (
            "unknown harness status",
            set_field(
                valid.clone(),
                "harness",
                json!({ "status": "exploded", "case_count": 1 }),
            ),
        ),
        (
            "skill missing description",
            set_field(
                valid.clone(),
                "skill",
                json!({ "id": "s1", "name": "Skill" }),
            ),
        ),
    ]
}

pub(super) fn review_receipt_output_corpus() -> Vec<(&'static str, Value)> {
    let valid = json!({
        "verdict": "needs_update",
        "failure_summary": "the harness step failed because the prompt drifted",
        "improvement_proposals": [{
            "target": "SKILL.md",
            "change": "tighten the output contract",
            "rationale": "prevents drift",
            "risk": "none",
        }],
        "next_harness_checks": ["output parses", "verdict present"],
    });
    vec![
        ("full valid", valid.clone()),
        ("valid pass (empty proposals)", {
            let mut v = valid.clone();
            v["verdict"] = json!("pass");
            v["improvement_proposals"] = json!([]);
            v
        }),
        ("valid with open extras", {
            let mut v = valid.clone();
            v["extra"] = json!("ok");
            v["improvement_proposals"] = json!([{ "target": "t", "change": "c", "extra": 1 }]);
            v
        }),
        ("missing verdict", drop_field(valid.clone(), "verdict")),
        (
            "missing failure_summary",
            drop_field(valid.clone(), "failure_summary"),
        ),
        (
            "missing next_harness_checks",
            drop_field(valid.clone(), "next_harness_checks"),
        ),
        (
            "unknown verdict",
            set_field(valid.clone(), "verdict", json!("maybe")),
        ),
        (
            "proposal missing change",
            set_field(
                valid.clone(),
                "improvement_proposals",
                json!([{ "target": "SKILL.md" }]),
            ),
        ),
        (
            "next_harness_checks as object",
            set_field(valid.clone(), "next_harness_checks", json!({})),
        ),
    ]
}

fn agent_context_meta() -> Value {
    json!({
        "artifact_id": "art_1",
        "run_id": "run_1",
        "step_id": null,
        "producer": { "skill": "demo", "runner": "local" },
        "created_at": "2026-01-01T00:00:00Z",
        "hash": "sha256:abc",
        "size_bytes": 12,
        "parent_artifact_id": null,
        "receipt_id": null,
        "redacted": false,
    })
}

pub(super) fn agent_context_envelope_corpus() -> Vec<(&'static str, Value)> {
    let valid = json!({
        "run_id": "run_1",
        "skill": "demo",
        "instructions": "do the thing",
        "inputs": {},
        "current_context": [],
        "historical_context": [],
        "provenance": [],
        "trust_boundary": "trusted",
    });
    vec![
        ("minimal valid", valid.clone()),
        ("full valid (nested context + output spec)", {
            let mut v = valid.clone();
            v["step_id"] = json!("step_1");
            v["current_context"] = json!([{
                "type": "artifact",
                "version": "1",
                "data": { "k": 1 },
                "meta": agent_context_meta(),
            }]);
            v["historical_context"] = json!([{
                "type": null,
                "version": "1",
                "data": {},
                "meta": agent_context_meta(),
            }]);
            v["provenance"] = json!([{ "input": "a", "output": "b", "from_step": "s0" }]);
            v["context"] = json!({
                "memory": { "root_path": "/r", "path": "MEMORY.md", "sha256": "abc", "content": "x" },
            });
            v["voice_profile"] =
                json!({ "root_path": "/r", "path": "voice.md", "sha256": "abc", "content": "" });
            v["execution_location"] =
                json!({ "skill_directory": "/skills/demo", "tool_roots": ["/tools"] });
            v["output"] =
                json!({ "result": "string", "report": { "type": "object", "required": true } });
            v
        }),
        ("missing run_id", drop_field(valid.clone(), "run_id")),
        (
            "missing trust_boundary",
            drop_field(valid.clone(), "trust_boundary"),
        ),
        (
            "empty run_id",
            set_field(valid.clone(), "run_id", json!("")),
        ),
        (
            "empty instructions",
            set_field(valid.clone(), "instructions", json!("")),
        ),
        (
            "context entry bad version const",
            set_field(
                valid.clone(),
                "current_context",
                json!([{ "type": "x", "version": "2", "data": {}, "meta": agent_context_meta() }]),
            ),
        ),
        (
            "context entry missing meta",
            set_field(
                valid.clone(),
                "current_context",
                json!([{ "type": "x", "version": "1", "data": {} }]),
            ),
        ),
        (
            "output bad type name",
            set_field(valid.clone(), "output", json!({ "result": "blob" })),
        ),
        (
            "additional property",
            set_field(valid.clone(), "bogus", json!(true)),
        ),
    ]
}

pub(super) fn receipt_corpus() -> Vec<(&'static str, Value)> {
    let valid = json!({
        "schema": "runx.receipt.v1",
        "id": "rcpt_1",
        "created_at": "2026-01-01T00:00:00Z",
        "canonicalization": "runx.receipt.c14n.v1",
        "issuer": {
            "type": "local",
            "kid": "fixture-key",
            "public_key_sha256": "sha256:abc",
        },
        "signature": { "alg": "Ed25519", "value": "sig:abc" },
        "digest": "sha256:abc",
        "idempotency": {
            "intent_key": "sha256:intent",
            "trigger_fingerprint": "sha256:trigger",
            "content_hash": "sha256:content",
        },
        "class": "executed",
        "subject": {
            "kind": "skill",
            "ref": a_ref(),
            "commitments": [],
        },
        "authority": {
            "actor_ref": a_ref(),
            "grant_refs": [],
            "scope_refs": [],
            "authority_proof_refs": [],
            "attenuation": { "parent_authority_ref": null, "subset_proof": null },
            "terms": [],
            "enforcement": {
                "profile_hash": "sha256:profile",
                "redaction_refs": [],
                "setup_refs": [],
                "teardown_refs": [],
            },
        },
        "signals": [],
        "decisions": [],
        "acts": [],
        "seal": {
            "disposition": "closed",
            "reason_code": "process_closed",
            "summary": "closed",
            "closed_at": "2026-01-01T00:00:00Z",
            "last_observed_at": "2026-01-01T00:00:00Z",
            "criteria": [],
        },
    });
    vec![
        ("minimal valid", valid.clone()),
        ("full valid (act + seal criteria + lineage)", {
            let mut v = valid.clone();
            v["acts"] = json!([{
                "id": "act_1",
                "form": "observation",
                "intent": an_intent(),
                "summary": "did the thing",
                "criterion_bindings": [{
                    "criterion_id": "c1",
                    "status": "verified",
                    "evidence_refs": [],
                    "verification_refs": [],
                    "summary": "ok",
                }],
                "source_refs": [],
                "target_refs": [],
                "artifact_refs": [],
                "closure": closed_act(),
            }]);
            v["seal"]["criteria"] = json!([{
                "criterion_id": "c1",
                "status": "verified",
                "evidence_refs": [],
                "verification_refs": [],
            }]);
            v["lineage"] = json!({
                "children": [],
                "sync": [],
            });
            v
        }),
        ("missing schema", drop_field(valid.clone(), "schema")),
        ("missing id", drop_field(valid.clone(), "id")),
        ("missing seal", drop_field(valid.clone(), "seal")),
        ("missing digest", drop_field(valid.clone(), "digest")),
        ("missing class", drop_field(valid.clone(), "class")),
        (
            "empty id rejected",
            set_field(valid.clone(), "id", json!("")),
        ),
        (
            "empty digest rejected",
            set_field(valid.clone(), "digest", json!("")),
        ),
        (
            "wrong schema const",
            set_field(valid.clone(), "schema", json!("runx.act.v1")),
        ),
        (
            "malformed created_at",
            set_field(valid.clone(), "created_at", json!("nope")),
        ),
        (
            "unknown issuer type",
            set_field(
                valid.clone(),
                "issuer",
                json!({ "type": "alien", "kid": "k", "public_key_sha256": "sha256:x" }),
            ),
        ),
        (
            "additional property",
            set_field(valid.clone(), "bogus", json!(true)),
        ),
    ]
}

fn closed_act() -> Value {
    json!({
        "disposition": "closed",
        "reason_code": "done",
        "summary": "completed",
        "closed_at": "2026-01-01T00:00:00Z",
    })
}

pub(super) fn act_corpus() -> Vec<(&'static str, Value)> {
    let valid = json!({
        "schema": "runx.act.v1",
        "act_id": "act_1",
        "form": "observation",
        "intent": an_intent(),
        "summary": "did the thing",
        "closure": closed_act(),
        "criterion_bindings": [],
        "source_refs": [],
        "target_refs": [],
        "surface_refs": [],
        "artifact_refs": [],
        "verification_refs": [],
        "harness_refs": [],
        "performed_at": "2026-01-01T00:00:00Z",
    });
    vec![
        ("minimal valid", valid.clone()),
        ("full valid (revision + bindings)", {
            let mut v = valid.clone();
            v["form"] = json!("revision");
            v["criterion_bindings"] = json!([{
                "criterion_id": "c1",
                "status": "verified",
                "evidence_refs": [],
                "verification_refs": [],
                "summary": "looks good",
            }]);
            v["revision"] = json!({
                "change_request": {
                    "request_id": "req_1",
                    "summary": "ship it",
                    "target_surfaces": [
                        { "surface_ref": a_ref(), "mutating": true, "rationale": "open pr" },
                    ],
                    "success_criteria": [],
                },
                "change_plan": {
                    "plan_id": "plan_1",
                    "summary": "open and merge",
                    "steps": ["open pr"],
                    "risks": [],
                },
                "target_surfaces": [],
                "invariants": ["keep tests green"],
                "handoff_refs": [],
                "revision_refs": [],
            });
            v
        }),
        (
            "missing schema (optional)",
            drop_field(valid.clone(), "schema"),
        ),
        ("missing act_id", drop_field(valid.clone(), "act_id")),
        ("missing closure", drop_field(valid.clone(), "closure")),
        (
            "missing performed_at",
            drop_field(valid.clone(), "performed_at"),
        ),
        (
            "empty act_id",
            set_field(valid.clone(), "act_id", json!("")),
        ),
        (
            "empty summary",
            set_field(valid.clone(), "summary", json!("")),
        ),
        (
            "unknown form",
            set_field(valid.clone(), "form", json!("teleport")),
        ),
        (
            "malformed performed_at",
            set_field(valid.clone(), "performed_at", json!("nope")),
        ),
        (
            "empty criterion binding criterion_id",
            set_field(
                valid.clone(),
                "criterion_bindings",
                json!([{
                    "criterion_id": "",
                    "status": "verified",
                    "evidence_refs": [],
                    "verification_refs": [],
                }]),
            ),
        ),
        (
            "additional property",
            set_field(valid.clone(), "bogus", json!(true)),
        ),
    ]
}

pub(super) fn operational_policy_corpus() -> Vec<(&'static str, Value)> {
    let valid = json!({
        "schema": "runx.operational_policy.v1",
        "schema_version": "runx.operational_policy.v1",
        "policy_id": "provider.intake",
        "sources": [{
            "source_id": "slack.intake",
            "provider": "slack",
            "allowed_locators": ["C123"],
            "allowed_actions": ["issue-intake"],
            "source_thread": {
                "required": true,
                "publish_mode": "reply",
                "missing_behavior": "fail_closed",
            },
        }],
        "runners": [{
            "runner_id": "local.default",
            "kind": "local",
            "state": "available",
            "allowed_actions": ["issue-intake"],
            "target_repos": ["acme/widgets"],
            "scafld_required": true,
        }],
        "owner_routes": [{
            "route_id": "default.route",
            "owners": ["alice"],
            "target_repos": ["acme/widgets"],
        }],
        "targets": [{
            "repo": "acme/widgets",
            "runner_ids": ["local.default"],
            "allowed_actions": ["issue-intake"],
            "default_owner_route": "default.route",
            "scafld_required": true,
        }],
        "dedupe": {
            "strategy": "source_fingerprint",
            "key_fields": ["source_id"],
            "on_duplicate": "reuse",
        },
        "outcomes": {
            "observe_provider": true,
            "verification_required": true,
            "close_source_issue": "when_verified",
            "publish_final_source_thread_update": true,
        },
        "permissions": {
            "auto_merge": false,
            "mutate_target_repo": true,
            "require_human_merge_gate": true,
        },
    });
    vec![
        ("minimal valid", valid.clone()),
        ("full valid (created_at + optionals)", {
            let mut v = valid.clone();
            v["created_at"] = json!("2026-01-01T00:00:00Z");
            v["sources"][0]["minimum_confidence"] = json!(0.5);
            v["sources"][0]["adapter_policy"] = json!({
                "sentry": { "production_only": true, "unresolved_only": true },
            });
            v["owner_routes"][0]["labels"] = json!(["bug"]);
            v["owner_routes"][0]["project"] = json!("Roadmap");
            v["targets"][0]["base_branch"] = json!("main");
            v
        }),
        ("missing schema", drop_field(valid.clone(), "schema")),
        (
            "missing schema_version",
            drop_field(valid.clone(), "schema_version"),
        ),
        ("missing policy_id", drop_field(valid.clone(), "policy_id")),
        ("missing dedupe", drop_field(valid.clone(), "dedupe")),
        (
            "empty policy_id rejected",
            set_field(valid.clone(), "policy_id", json!("")),
        ),
        (
            "wrong schema const",
            set_field(valid.clone(), "schema", json!("runx.other.v1")),
        ),
        (
            "unknown dedupe strategy",
            set_field(
                valid.clone(),
                "dedupe",
                json!({
                    "strategy": "magic",
                    "key_fields": ["source_id"],
                    "on_duplicate": "reuse",
                }),
            ),
        ),
        (
            "malformed created_at",
            set_field(valid.clone(), "created_at", json!("nope")),
        ),
        (
            "additional property",
            set_field(valid.clone(), "bogus", json!(true)),
        ),
    ]
}

pub(super) fn operational_proposal_corpus() -> Vec<(&'static str, Value)> {
    let valid = json!({
        "schema": "runx.operational_proposal.v1",
        "proposal_id": "proposal_123",
        "proposal_kind": "escalation",
        "source_event_id": "slack_event_123",
        "idempotency": {
            "key": "operational-proposal:slack_event_123:tracking-to-change:api-owner",
            "fingerprint": "sha256:proposal-123-source-action-target",
        },
        "source_ref": {
            "type": "thread",
            "uri": "slack://team/T123/channel/CBUGS/thread/1710000000.000100",
        },
        "source_thread_ref": {
            "type": "thread",
            "uri": "slack://team/T123/channel/CBUGS/thread/1710000000.000100",
        },
        "hydrated_context_ref": {
            "type": "artifact",
            "uri": "runx:artifact:hydrated_context_123",
        },
        "redaction_status": "redacted",
        "decision_summary": "Prepare a governed fix.",
        "rationale": "The source thread has enough evidence for a bounded change.",
        "recommended_actions": [{
            "action_intent": "tracking-to-change",
            "summary": "Build the fix in the owning repository.",
            "mutating": true,
            "target_refs": [{
                "type": "repository",
                "uri": "github://example/api",
            }],
        }],
        "evidence_refs": [{
            "type": "artifact",
            "uri": "runx:artifact:evidence_123",
        }],
        "artifact_refs": [{
            "type": "artifact",
            "uri": "runx:artifact:proposal_123",
        }],
        "receipt_refs": [{
            "type": "receipt",
            "uri": "runx:receipt:receipt_123",
        }],
        "story_refs": [{
            "type": "surface",
            "uri": "runx:story:story_123",
        }],
        "result_refs": [{
            "role": "tracking_item",
            "ref": {
                "type": "tracking_item",
                "uri": "github://example/api/issues/123",
            },
        }, {
            "role": "change_request",
            "ref": {
                "type": "change_request",
                "uri": "github://example/api/pulls/124",
            },
        }],
        "publication_refs": [{
            "role": "source_thread_update",
            "ref": {
                "type": "thread",
                "uri": "slack://team/T123/channel/CBUGS/thread/1710000000.000100",
            },
        }, {
            "role": "tracking_item_comment",
            "ref": {
                "type": "comment",
                "uri": "https://github.com/example/api/issues/123#issuecomment-1",
            },
        }],
        "owner_route_id": "api-owner",
        "confidence": 0.82,
        "risks": ["The issue may be broader than the selected target."],
        "caveats": ["Human merge remains required."],
        "missing_context": [],
        "authority": {
            "proposal_only": true,
            "mutation_authority_granted": false,
            "publication_authority_granted": false,
            "final_decision_authority_granted": false,
            "notes": ["Public thread updates are allowed; merge is not."],
        },
        "human_gates": [{
            "gate_id": "gate_merge_review",
            "gate_kind": "final_change_approval",
            "required": true,
            "decision": "Review and approve the final change only if it is correct.",
            "reason": "Target repo mutation requires a human final-change gate.",
        }],
        "allowed_next_actions": ["tracking-to-change", "manual-review"],
        "final_outcome": {
            "observed": true,
            "status": "merged",
            "summary": "The governed change request was merged and verified.",
            "observed_at": "2026-05-28T00:00:00Z",
            "refs": [{
                "type": "change_request",
                "uri": "github://example/api/pulls/124",
            }],
        },
        "public_summary": "Governed fix proposal with tracking, change request, and final outcome links.",
    });
    vec![
        ("minimal valid", valid.clone()),
        ("valid blocked proposal", {
            let mut v = valid.clone();
            v["proposal_id"] = json!("proposal_blocked");
            v["proposal_kind"] = json!("support_reply");
            v["redaction_status"] = json!("summary_only");
            v["recommended_actions"][0]["action_intent"] = json!("reply-only");
            v["recommended_actions"][0]["mutating"] = json!(false);
            v["human_gates"][0]["gate_kind"] = json!("customer_send_approval");
            remove_field(&mut v, "result_refs");
            remove_field(&mut v, "final_outcome");
            v
        }),
        (
            "missing source ref",
            drop_field(valid.clone(), "source_ref"),
        ),
        (
            "missing redaction status",
            drop_field(valid.clone(), "redaction_status"),
        ),
        (
            "unapproved mutation authority",
            set_field(
                valid.clone(),
                "authority",
                set_field(
                    valid["authority"].clone(),
                    "mutation_authority_granted",
                    json!(true),
                ),
            ),
        ),
        (
            "unapproved final decision authority",
            set_field(
                valid.clone(),
                "authority",
                set_field(
                    valid["authority"].clone(),
                    "final_decision_authority_granted",
                    json!(true),
                ),
            ),
        ),
        (
            "bad confidence",
            set_field(valid.clone(), "confidence", json!(2)),
        ),
        (
            "provider-specific top-level field",
            set_field(
                valid.clone(),
                "provider_specific_issue_url",
                json!("https://provider.example/work/items/123"),
            ),
        ),
        (
            "additional property",
            set_field(valid.clone(), "raw_payload", json!({"secret": true})),
        ),
    ]
}

fn authority_term() -> Value {
    json!({
        "term_id": "term_1",
        "principal_ref": a_ref(),
        "resource_ref": a_ref(),
        "resource_family": "connector",
        "verbs": ["read", "write"],
        "bounds": {},
        "conditions": [],
        "approvals": [],
        "capabilities": [],
        "issued_by_ref": a_ref(),
    })
}

pub(super) fn orchestrator_execution_context_corpus() -> Vec<(&'static str, Value)> {
    vec![
        ("caller origin", json!({ "caller": "runx-cli" })),
        (
            "full binding",
            json!({
                "workflow_ref": "workflow:demo",
                "platform": "n8n",
                "event_id": "evt_demo_001",
                "idempotency_key": "evt_demo_001",
                "handoff_scope": "orchestrator.n8n.workflow.invoke",
                "handoff_audience": "n8n:workflow:demo",
                "environment": "local",
            }),
        ),
        (
            "provider extension",
            json!({ "principal": "operator:1", "trace_id": 42 }),
        ),
        ("invalid known field type", json!({ "caller": 42 })),
    ]
}

pub(super) fn orchestrator_handoff_context_corpus() -> Vec<(&'static str, Value)> {
    let valid = json!({
        "status": "ready",
        "platform": "n8n",
        "event_id": "evt_demo_001",
        "idempotency": { "key": "evt_demo_001", "receiver_should_dedupe": true },
        "handoff": {
            "scope": "orchestrator.n8n.workflow.invoke",
            "audience": "n8n:workflow:demo",
            "source": "runx",
        },
        "delivery": {
            "event_id": "evt_demo_001",
            "handoff_scope": "orchestrator.n8n.workflow.invoke",
            "handoff_audience": "n8n:workflow:demo",
            "execution_context": { "caller": "runx-cli", "platform": "n8n" },
            "payload": { "hello": "workflow" },
            "source": "runx",
            "idempotency_key": "evt_demo_001",
        },
        "receiver_validation": {
            "require_scope": "orchestrator.n8n.workflow.invoke",
            "require_audience": "n8n:workflow:demo",
            "require_event_id": "evt_demo_001",
            "reject_duplicate_event_id": true,
        },
        "receipt_expectations": {
            "context_artifact": "handoff_context",
            "outbound_effect_must_be_receipted": true,
            "receiver_response_must_be_captured": true,
            "delivered_credential_material_absent": true,
        },
    });
    vec![
        ("valid preflight", valid.clone()),
        ("missing delivery", drop_field(valid.clone(), "delivery")),
        (
            "invalid delivery payload source type",
            set_field(valid.clone(), "delivery", json!({ "source": 42 })),
        ),
        (
            "additional property",
            set_field(valid, "credential", json!("secret")),
        ),
    ]
}

pub(super) fn authority_term_corpus() -> Vec<(&'static str, Value)> {
    let valid = authority_term();
    vec![
        ("minimal valid", valid.clone()),
        ("missing term_id", drop_field(valid.clone(), "term_id")),
        (
            "empty term_id rejected",
            set_field(valid.clone(), "term_id", json!("")),
        ),
        (
            "unknown resource family",
            set_field(valid.clone(), "resource_family", json!("payment")),
        ),
        (
            "additional property",
            set_field(valid, "authority_ref", json!("authority:payment:test")),
        ),
    ]
}

pub(super) fn authority_corpus() -> Vec<(&'static str, Value)> {
    let valid = json!({
        "schema": "runx.authority.v1",
        "actor_ref": a_ref(),
        "authority_proof_refs": [],
        "grant_refs": [],
        "scope_refs": [],
        "policy_refs": [],
        "terms": [authority_term()],
        "attenuation": {
            "parent_authority_ref": null,
            "subset_proof": null,
        },
    });
    vec![
        ("minimal valid (nullable attenuation)", valid.clone()),
        ("full valid", {
            let mut v = valid.clone();
            v["attenuation"] = json!({
                "parent_authority_ref": a_ref(),
                "subset_proof": {
                    "parent_authority_ref": a_ref(),
                    "comparison_algorithm": "runx.subset.v1",
                    "result": "subset",
                    "compared_terms": [
                        { "child_term_id": "c1", "parent_term_id": "p1", "relation": "subset" },
                    ],
                    "checked_at": "2026-01-01T00:00:00Z",
                },
            });
            v["mandate_ref"] = a_ref();
            v["terms"] = json!([{
                "term_id": "term_1",
                "principal_ref": a_ref(),
                "resource_ref": a_ref(),
                "resource_family": "effect",
                "verbs": ["commit"],
                "bounds": {
                    "effect_limits": [{
                        "family": "payment",
                        "unit": "USD",
                        "channels": ["card"],
                        "max_per_call_units": 2500,
                    }],
                },
                "conditions": [
                    { "condition_id": "cond_1", "predicate": "within_budget" },
                ],
                "approvals": [
                    { "approval_ref": a_ref(), "approved_at": "2026-01-01T00:00:00Z" },
                ],
                "capabilities": ["effect_single_use_capability"],
                "expires_at": "2026-02-01T00:00:00Z",
                "issued_by_ref": a_ref(),
            }]);
            v
        }),
        ("missing actor_ref", drop_field(valid.clone(), "actor_ref")),
        (
            "missing attenuation",
            drop_field(valid.clone(), "attenuation"),
        ),
        (
            "empty term_id rejected",
            set_field(
                valid.clone(),
                "terms",
                json!([set_field(authority_term(), "term_id", json!(""))]),
            ),
        ),
        (
            "unknown resource_family",
            set_field(
                valid.clone(),
                "terms",
                json!([set_field(
                    authority_term(),
                    "resource_family",
                    json!("nope")
                )]),
            ),
        ),
        (
            "unknown verb",
            set_field(
                valid.clone(),
                "terms",
                json!([set_field(authority_term(), "verbs", json!(["fly"]))]),
            ),
        ),
        (
            "empty effect limit family rejected",
            set_field(
                valid.clone(),
                "terms",
                json!([set_field(
                    authority_term(),
                    "bounds",
                    json!({ "effect_limits": [{ "family": "", "unit": "USD", "channels": ["card"] }] }),
                )]),
            ),
        ),
        (
            "malformed approval approved_at",
            set_field(
                valid.clone(),
                "terms",
                json!([set_field(
                    authority_term(),
                    "approvals",
                    json!([{ "approval_ref": a_ref(), "approved_at": "nope" }]),
                )]),
            ),
        ),
        (
            "additional property",
            set_field(valid.clone(), "bogus", json!(true)),
        ),
    ]
}

pub(super) fn authority_subset_proof_corpus() -> Vec<(&'static str, Value)> {
    let valid = json!({
        "parent_authority_ref": a_ref(),
        "comparison_algorithm": "runx.subset.v1",
        "result": "subset",
        "compared_terms": [
            { "child_term_id": "c1", "parent_term_id": "p1", "relation": "subset" },
        ],
        "checked_at": "2026-01-01T00:00:00Z",
    });
    vec![
        ("minimal valid", valid.clone()),
        (
            "with proof_ref",
            set_field(valid.clone(), "proof_ref", a_ref()),
        ),
        (
            "missing comparison_algorithm",
            drop_field(valid.clone(), "comparison_algorithm"),
        ),
        ("missing result", drop_field(valid.clone(), "result")),
        (
            "missing checked_at",
            drop_field(valid.clone(), "checked_at"),
        ),
        (
            "empty comparison_algorithm",
            set_field(valid.clone(), "comparison_algorithm", json!("")),
        ),
        (
            "unknown result value",
            set_field(valid.clone(), "result", json!("superset")),
        ),
        (
            "comparison empty child_term_id",
            set_field(
                valid.clone(),
                "compared_terms",
                json!([{ "child_term_id": "", "parent_term_id": "p1", "relation": "subset" }]),
            ),
        ),
        (
            "comparison unknown relation",
            set_field(
                valid.clone(),
                "compared_terms",
                json!([{ "child_term_id": "c1", "parent_term_id": "p1", "relation": "disjoint" }]),
            ),
        ),
        (
            "malformed checked_at",
            set_field(valid.clone(), "checked_at", json!("nope")),
        ),
        (
            "additional property",
            set_field(valid.clone(), "bogus", json!(true)),
        ),
    ]
}

pub(super) fn act_assignment_corpus() -> Vec<(&'static str, Value)> {
    let valid = json!({
        "schema": "runx.act_assignment.v1",
        "skill_ref": "skill:1",
        "runner": "local",
        "requested_at": "2026-01-01T00:00:00Z",
        "host": { "kind": "cli" },
        "idempotency": {
            "algorithm": "sha256",
            "intent_key": "sha256:intent",
            "content_hash": "sha256:content",
        },
    });
    vec![
        ("minimal valid", valid.clone()),
        ("full valid", {
            let mut v = valid.clone();
            v["source_ref"] = json!("runx:signal:1");
            v["input_overrides"] = json!({ "k": 1 });
            v["host"] = json!({
                "kind": "github_issue_comment",
                "trigger_ref": "owner/repo#1",
                "scope_set": ["issues:write"],
                "actor": { "actor_id": "u1", "display_name": "User" },
            });
            v["idempotency"] = json!({
                "algorithm": "sha256",
                "intent_key": "sha256:intent",
                "trigger_key": "sha256:trigger",
                "content_hash": "sha256:content",
            });
            v
        }),
        ("missing skill_ref", drop_field(valid.clone(), "skill_ref")),
        ("missing schema", drop_field(valid.clone(), "schema")),
        (
            "missing idempotency",
            drop_field(valid.clone(), "idempotency"),
        ),
        (
            "empty skill_ref",
            set_field(valid.clone(), "skill_ref", json!("")),
        ),
        (
            "empty runner",
            set_field(valid.clone(), "runner", json!("")),
        ),
        (
            "unknown host kind",
            set_field(valid.clone(), "host", json!({ "kind": "carrier-pigeon" })),
        ),
        (
            "malformed requested_at",
            set_field(valid.clone(), "requested_at", json!("nope")),
        ),
        (
            "additional property",
            set_field(valid.clone(), "bogus", json!(true)),
        ),
    ]
}

fn drop_field(mut value: Value, field: &str) -> Value {
    remove_field(&mut value, field);
    value
}

fn remove_field(value: &mut Value, field: &str) {
    if let Some(object) = value.as_object_mut() {
        object.remove(field);
    }
}

pub(super) fn set_field(mut value: Value, field: &str, replacement: Value) -> Value {
    value[field] = replacement;
    value
}

pub(super) fn credential_delivery_profile_corpus() -> Vec<(&'static str, Value)> {
    let valid = json!({
        "schema": "runx.credential_delivery.profile.v1",
        "profile_id": "github-env",
        "provider": "github",
        "auth_mode": "api_key",
        "purpose": "provider_api",
        "delivery_mode": "process_env",
        "material_roles": ["api_key"],
        "env_bindings": [{ "role": "api_key", "env_var": "GITHUB_TOKEN", "required": true }],
        "redaction_policy_ref": a_ref(),
    });
    vec![
        ("valid", valid.clone()),
        (
            "missing profile_id",
            drop_field(valid.clone(), "profile_id"),
        ),
        ("missing schema", drop_field(valid.clone(), "schema")),
        (
            "empty profile_id",
            set_field(valid.clone(), "profile_id", json!("")),
        ),
        (
            "empty provider",
            set_field(valid.clone(), "provider", json!("")),
        ),
        (
            "unknown purpose",
            set_field(valid.clone(), "purpose", json!("nope")),
        ),
        (
            "unknown delivery_mode",
            set_field(valid.clone(), "delivery_mode", json!("nope")),
        ),
        (
            "unknown material role",
            set_field(valid.clone(), "material_roles", json!(["nope"])),
        ),
        (
            "additional property",
            set_field(valid.clone(), "bogus", json!(true)),
        ),
    ]
}

pub(super) fn credential_delivery_request_corpus() -> Vec<(&'static str, Value)> {
    let valid = json!({
        "schema": "runx.credential_delivery.request.v1",
        "request_id": "req_1",
        "harness_ref": a_ref(),
        "host_ref": a_ref(),
        "grant_ref": a_ref(),
        "credential_ref": a_ref(),
        "profile_id": "github-env",
        "provider": "github",
        "purpose": "provider_api",
        "requested_roles": ["api_key"],
        "requested_at": "2026-01-01T00:00:00Z",
    });
    vec![
        ("valid", valid.clone()),
        (
            "missing request_id",
            drop_field(valid.clone(), "request_id"),
        ),
        ("missing schema", drop_field(valid.clone(), "schema")),
        (
            "empty request_id",
            set_field(valid.clone(), "request_id", json!("")),
        ),
        (
            "empty profile_id",
            set_field(valid.clone(), "profile_id", json!("")),
        ),
        (
            "unknown purpose",
            set_field(valid.clone(), "purpose", json!("nope")),
        ),
        (
            "malformed requested_at",
            set_field(valid.clone(), "requested_at", json!("nope")),
        ),
        (
            "missing requested_at",
            drop_field(valid.clone(), "requested_at"),
        ),
        (
            "additional property",
            set_field(valid.clone(), "bogus", json!(true)),
        ),
    ]
}

pub(super) fn credential_delivery_response_corpus() -> Vec<(&'static str, Value)> {
    let valid = json!({
        "schema": "runx.credential_delivery.response.v1",
        "response_id": "resp_1",
        "request_id": "req_1",
        "status": "delivered",
        "credential_refs": [a_ref()],
        "issued_at": "2026-01-01T00:00:00Z",
    });
    vec![
        ("minimal valid", valid.clone()),
        ("full valid", {
            let mut v = valid.clone();
            v["delivery_mode"] = json!("process_env");
            v["handles"] = json!([{ "role": "api_key", "delivery_handle_ref": a_ref(), "env_var": "GITHUB_TOKEN" }]);
            v["material_ref_hash"] = json!("sha256:abc");
            v["denied_reasons"] = json!([]);
            v["expires_at"] = json!("2026-02-01T00:00:00Z");
            v
        }),
        (
            "missing response_id",
            drop_field(valid.clone(), "response_id"),
        ),
        ("missing schema", drop_field(valid.clone(), "schema")),
        (
            "empty response_id",
            set_field(valid.clone(), "response_id", json!("")),
        ),
        (
            "unknown status",
            set_field(valid.clone(), "status", json!("nope")),
        ),
        (
            "empty denied_reasons item",
            set_field(valid.clone(), "denied_reasons", json!([""])),
        ),
        (
            "malformed issued_at",
            set_field(valid.clone(), "issued_at", json!("nope")),
        ),
        (
            "additional property",
            set_field(valid.clone(), "bogus", json!(true)),
        ),
    ]
}

pub(super) fn credential_delivery_observation_corpus() -> Vec<(&'static str, Value)> {
    let valid = json!({
        "schema": "runx.credential_delivery.observation.v1",
        "observation_id": "obs_1",
        "request_id": "req_1",
        "status": "delivered",
        "harness_ref": a_ref(),
        "profile_id": "github-env",
        "provider": "github",
        "purpose": "provider_api",
        "credential_refs": [a_ref()],
        "delivered_roles": ["api_key"],
        "observed_at": "2026-01-01T00:00:00Z",
    });
    vec![
        ("minimal valid", valid.clone()),
        (
            "missing observation_id",
            drop_field(valid.clone(), "observation_id"),
        ),
        ("missing schema", drop_field(valid.clone(), "schema")),
        (
            "empty observation_id",
            set_field(valid.clone(), "observation_id", json!("")),
        ),
        (
            "empty profile_id",
            set_field(valid.clone(), "profile_id", json!("")),
        ),
        (
            "unknown status",
            set_field(valid.clone(), "status", json!("nope")),
        ),
        (
            "malformed observed_at",
            set_field(valid.clone(), "observed_at", json!("nope")),
        ),
        (
            "missing harness_ref",
            drop_field(valid.clone(), "harness_ref"),
        ),
        (
            "additional property",
            set_field(valid.clone(), "bogus", json!(true)),
        ),
    ]
}

pub(super) fn question_corpus() -> Vec<(&'static str, Value)> {
    let valid = json!({ "id": "q1", "prompt": "What?", "required": true, "type": "string" });
    vec![
        ("minimal valid", valid.clone()),
        (
            "full valid",
            set_field(valid.clone(), "description", json!("a hint")),
        ),
        ("missing id", drop_field(valid.clone(), "id")),
        ("missing prompt", drop_field(valid.clone(), "prompt")),
        ("missing type", drop_field(valid.clone(), "type")),
        ("empty id", set_field(valid.clone(), "id", json!(""))),
        (
            "empty prompt",
            set_field(valid.clone(), "prompt", json!("")),
        ),
        ("empty type", set_field(valid.clone(), "type", json!(""))),
        (
            "required as string",
            set_field(valid.clone(), "required", json!("yes")),
        ),
        (
            "additional property",
            set_field(valid.clone(), "bogus", json!(true)),
        ),
    ]
}

pub(super) fn approval_gate_corpus() -> Vec<(&'static str, Value)> {
    let valid = json!({ "id": "g1", "reason": "needs approval" });
    vec![
        ("minimal valid", valid.clone()),
        ("full valid", {
            let mut v = valid.clone();
            v["type"] = json!("filesystem_write");
            v["summary"] = json!({ "k": 1 });
            v
        }),
        ("missing id", drop_field(valid.clone(), "id")),
        ("missing reason", drop_field(valid.clone(), "reason")),
        ("empty id", set_field(valid.clone(), "id", json!(""))),
        (
            "empty reason",
            set_field(valid.clone(), "reason", json!("")),
        ),
        (
            "summary as array",
            set_field(valid.clone(), "summary", json!([1, 2])),
        ),
        (
            "additional property",
            set_field(valid.clone(), "bogus", json!(true)),
        ),
    ]
}

pub(super) fn resolution_response_corpus() -> Vec<(&'static str, Value)> {
    let valid = json!({ "actor": "human", "payload": { "answer": "yes" } });
    vec![
        ("valid human", valid.clone()),
        (
            "valid agent",
            set_field(valid.clone(), "actor", json!("agent")),
        ),
        (
            "payload as string accepted",
            set_field(valid.clone(), "payload", json!("text")),
        ),
        (
            "payload as null accepted",
            set_field(valid.clone(), "payload", json!(null)),
        ),
        ("missing actor", drop_field(valid.clone(), "actor")),
        ("missing payload", drop_field(valid.clone(), "payload")),
        (
            "unknown actor",
            set_field(valid.clone(), "actor", json!("robot")),
        ),
        (
            "additional property",
            set_field(valid.clone(), "bogus", json!(true)),
        ),
    ]
}

pub(super) fn agent_act_invocation_corpus() -> Vec<(&'static str, Value)> {
    let valid = json!({
        "id": "inv_1",
        "source_type": "agent-task",
        "agent": "assistant",
        "task": "draft",
        "envelope": agent_context_envelope(),
        "allowance": {},
    });
    vec![
        ("valid agent-task invocation", valid.clone()),
        (
            "valid turn allowance",
            set_field(
                valid.clone(),
                "allowance",
                json!({
                    "operations": [{"requirement": "github", "operation": "issue.read"}],
                    "tools": ["fs.read"],
                    "skills": ["./triage"],
                    "asks": true,
                }),
            ),
        ),
        (
            "allowance asks not a boolean",
            set_field(valid.clone(), "allowance", json!({"asks": "yes"})),
        ),
        ("missing allowance", drop_field(valid.clone(), "allowance")),
        (
            "allowance empty tool",
            set_field(valid.clone(), "allowance", json!({"tools": [""]})),
        ),
        (
            "allowance operation without requirement",
            set_field(
                valid.clone(),
                "allowance",
                json!({"operations": [{"operation": "issue.read"}]}),
            ),
        ),
        (
            "valid agent invocation",
            set_field(valid.clone(), "source_type", json!("agent")),
        ),
        ("missing id", drop_field(valid.clone(), "id")),
        ("empty id", set_field(valid.clone(), "id", json!(""))),
        (
            "unknown source type",
            set_field(valid.clone(), "source_type", json!("robot")),
        ),
        (
            "empty optional agent",
            set_field(valid.clone(), "agent", json!("")),
        ),
        ("missing envelope", drop_field(valid.clone(), "envelope")),
        (
            "invalid envelope",
            set_field(
                valid.clone(),
                "envelope",
                drop_field(agent_context_envelope(), "trust_boundary"),
            ),
        ),
        (
            "additional property",
            set_field(valid.clone(), "bogus", json!(true)),
        ),
    ]
}

fn agent_context_envelope() -> Value {
    json!({
        "run_id": "run_1",
        "skill": "demo",
        "instructions": "do the thing",
        "inputs": {},
        "current_context": [],
        "historical_context": [],
        "provenance": [],
        "trust_boundary": "trusted",
    })
}

fn turn_invocation() -> Value {
    json!({
        "id": "agent.answer.output",
        "source_type": "agent",
        "envelope": agent_context_envelope(),
        "allowance": { "tools": ["data.digest"], "asks": true },
    })
}

fn turn_call() -> Value {
    json!({
        "index": 0,
        "target": { "kind": "tool", "tool": "data.digest" },
        "outcome": {
            "status": "completed",
            "output": { "digest": "sha256:abc" },
            "receipt_id": "rx_call",
        },
    })
}

pub(super) fn turn_call_corpus() -> Vec<(&'static str, Value)> {
    let valid = turn_call();
    vec![
        ("valid completed tool call", valid.clone()),
        (
            "valid refused operation call",
            json!({
                "index": 1,
                "target": { "kind": "operation", "requirement": "mail", "operation": "send" },
                "outcome": { "status": "refused", "reason": "not in the turn's allowance" },
            }),
        ),
        (
            "valid failed skill call without receipt",
            json!({
                "index": 2,
                "target": { "kind": "skill", "skill": "./triage" },
                "outcome": { "status": "failed", "reason": "the child skill failed" },
            }),
        ),
        (
            "valid pending ask",
            json!({
                "index": 3,
                "target": { "kind": "ask" },
                "outcome": {
                    "status": "pending",
                    "waiting": [{
                        "kind": "approval",
                        "id": "answer/3",
                        "gate": { "id": "answer/3", "reason": "Send it?" },
                    }],
                },
            }),
        ),
        (
            "valid pending skill call waiting on its child's turn",
            json!({
                "index": 4,
                "target": { "kind": "skill", "skill": "./report" },
                "outcome": {
                    "status": "pending",
                    "waiting": [{
                        "kind": "agent_act",
                        "id": "answer/4#agent.report.output",
                        "invocation": turn_invocation(),
                    }],
                },
            }),
        ),
        ("missing outcome", drop_field(valid.clone(), "outcome")),
        (
            "unknown target kind",
            set_field(valid.clone(), "target", json!({ "kind": "shell" })),
        ),
        (
            "tool target without its tool",
            set_field(valid.clone(), "target", json!({ "kind": "tool" })),
        ),
        (
            "completed without receipt",
            set_field(
                valid.clone(),
                "outcome",
                json!({ "status": "completed", "output": {} }),
            ),
        ),
        (
            "refused with empty reason",
            set_field(
                valid.clone(),
                "outcome",
                json!({ "status": "refused", "reason": "" }),
            ),
        ),
        (
            "pending without what it waits on",
            set_field(valid.clone(), "outcome", json!({ "status": "pending" })),
        ),
        (
            "additional property",
            set_field(valid.clone(), "bogus", json!(true)),
        ),
    ]
}

pub(super) fn turn_status_corpus() -> Vec<(&'static str, Value)> {
    let valid = json!({
        "invocation": turn_invocation(),
        "tools": [{
            "name": "data_digest",
            "description": "Digest a value.",
            "target": { "kind": "tool", "tool": "data.digest" },
            "input_schema": { "type": "object" },
        }],
        "calls": [turn_call()],
    });
    vec![
        ("valid status", valid.clone()),
        (
            "valid status without calls",
            set_field(valid.clone(), "calls", json!([])),
        ),
        (
            "missing invocation",
            drop_field(valid.clone(), "invocation"),
        ),
        ("missing tools", drop_field(valid.clone(), "tools")),
        ("missing calls", drop_field(valid.clone(), "calls")),
        (
            "tool without its target",
            set_field(
                valid.clone(),
                "tools",
                json!([{ "name": "data_digest", "description": "Digest a value.", "input_schema": {} }]),
            ),
        ),
        (
            "invalid call",
            set_field(valid.clone(), "calls", json!([{ "index": 0 }])),
        ),
        (
            "additional property",
            set_field(valid.clone(), "run_id", json!("rx_1")),
        ),
    ]
}

pub(super) fn resolution_request_corpus() -> Vec<(&'static str, Value)> {
    let input = json!({
        "kind": "input",
        "id": "q_1",
        "questions": [{ "id": "name", "prompt": "Name?", "required": true, "type": "string" }],
    });
    let approval = json!({
        "kind": "approval",
        "id": "ap_1",
        "gate": { "id": "g1", "reason": "needs approval" },
    });
    let agent_act = json!({
        "kind": "agent_act",
        "id": "aa_1",
        "invocation": {
            "id": "inv_1",
            "source_type": "agent",
            "envelope": agent_context_envelope(),
            "allowance": {},
        },
    });
    vec![
        ("valid input request", input.clone()),
        ("valid approval request", approval.clone()),
        ("valid agent_act request", agent_act.clone()),
        (
            "input missing questions",
            drop_field(input.clone(), "questions"),
        ),
        (
            "input empty id rejected",
            set_field(input.clone(), "id", json!("")),
        ),
        (
            "unknown kind rejected",
            set_field(input.clone(), "kind", json!("teleport")),
        ),
        (
            "approval empty gate reason rejected",
            set_field(
                approval.clone(),
                "gate",
                json!({ "id": "g1", "reason": "" }),
            ),
        ),
        (
            "approval missing gate",
            drop_field(approval.clone(), "gate"),
        ),
        (
            "agent_act missing invocation",
            drop_field(agent_act.clone(), "invocation"),
        ),
        (
            "input additional property rejected",
            set_field(input.clone(), "bogus", json!(true)),
        ),
        (
            "question additional property rejected",
            set_field(
                input.clone(),
                "questions",
                json!([{ "id": "name", "prompt": "Name?", "required": true, "type": "string", "bogus": 1 }]),
            ),
        ),
    ]
}

fn an_intent() -> Value {
    json!({
        "purpose": "ship the change",
        "legitimacy": "operator approved",
        "success_criteria": [],
        "constraints": [],
        "derived_from": [],
    })
}

pub(super) fn decision_corpus() -> Vec<(&'static str, Value)> {
    let inputs = json!({
        "signal_refs": [],
        "target_ref": null,
        "opportunity_refs": [],
        "selection_ref": null,
    });
    let valid = json!({
        "schema": "runx.decision.v1",
        "decision_id": "dec_1",
        "choice": "open",
        "inputs": inputs.clone(),
        "proposed_intent": an_intent(),
        "selected_act_id": null,
        "selected_harness_ref": null,
        "justification": { "summary": "because", "evidence_refs": [] },
        "closure": null,
        "artifact_refs": [],
    });
    vec![
        ("valid with all nullables null", valid.clone()),
        ("valid with nullables populated", {
            let mut v = valid.clone();
            v["selected_act_id"] = json!("act_1");
            v["selected_harness_ref"] = a_ref();
            v["closure"] = json!({
                "disposition": "closed",
                "reason_code": "done",
                "summary": "completed",
                "closed_at": "2026-01-01T00:00:00Z",
            });
            v["inputs"] = json!({
                "signal_refs": [a_ref()],
                "target_ref": a_ref(),
                "opportunity_refs": [],
                "selection_ref": a_ref(),
            });
            v
        }),
        ("missing required-but-nullable selected_act_id", {
            let mut v = valid.clone();
            remove_field(&mut v, "selected_act_id");
            v
        }),
        ("missing required-but-nullable closure", {
            let mut v = valid.clone();
            remove_field(&mut v, "closure");
            v
        }),
        ("missing required inputs.target_ref", {
            let mut v = valid.clone();
            v["inputs"] = json!({
                "signal_refs": [],
                "opportunity_refs": [],
                "selection_ref": null,
            });
            v
        }),
        ("empty decision_id rejected", {
            let mut v = valid.clone();
            v["decision_id"] = json!("");
            v
        }),
        ("empty selected_act_id rejected by minLength", {
            let mut v = valid.clone();
            v["selected_act_id"] = json!("");
            v
        }),
        ("unknown choice variant", {
            let mut v = valid.clone();
            v["choice"] = json!("ponder");
            v
        }),
        ("malformed closure closed_at", {
            let mut v = valid.clone();
            v["closure"] = json!({
                "disposition": "closed",
                "reason_code": "done",
                "summary": "completed",
                "closed_at": "nope",
            });
            v
        }),
        ("additional property", {
            let mut v = valid.clone();
            v["bogus"] = json!(true);
            v
        }),
    ]
}

pub(super) fn verification_corpus() -> Vec<(&'static str, Value)> {
    let check = json!({
        "check_id": "c1",
        "criterion_ids": ["crit_1"],
        "status": "passed",
        "summary": "looks good",
        "checked_refs": [a_ref()],
        "evidence_refs": [a_ref()],
        "verified_at": "2026-01-01T00:00:00Z",
    });
    let valid = json!({
        "schema": "runx.verification.v1",
        "verification_id": "ver_1",
        "status": "passed",
        "checks": [check],
        "verified_at": "2026-01-01T00:00:00Z",
        "evidence_refs": [a_ref()],
    });
    vec![
        ("full valid", valid.clone()),
        (
            "minimal valid",
            json!({ "status": "pending", "checks": [], "evidence_refs": [] }),
        ),
        (
            "valid without optional schema marker and id",
            json!({
                "status": "failed",
                "checks": [{
                    "check_id": "c1",
                    "criterion_ids": ["crit_1"],
                    "status": "failed",
                    "evidence_refs": [],
                }],
                "evidence_refs": [],
            }),
        ),
        ("missing status", {
            let mut v = valid.clone();
            remove_field(&mut v, "status");
            v
        }),
        ("missing checks", {
            let mut v = valid.clone();
            remove_field(&mut v, "checks");
            v
        }),
        ("missing evidence_refs", {
            let mut v = valid.clone();
            remove_field(&mut v, "evidence_refs");
            v
        }),
        ("unknown status variant", {
            let mut v = valid.clone();
            v["status"] = json!("maybe");
            v
        }),
        ("empty verification_id", {
            let mut v = valid.clone();
            v["verification_id"] = json!("");
            v
        }),
        ("malformed verified_at", {
            let mut v = valid.clone();
            v["verified_at"] = json!("not-a-timestamp");
            v
        }),
        ("check missing required field", {
            let mut v = valid.clone();
            v["checks"] = json!([{ "criterion_ids": ["crit_1"], "status": "passed" }]);
            v
        }),
        ("additional property", {
            let mut v = valid.clone();
            v["bogus"] = json!(true);
            v
        }),
    ]
}

pub(super) fn signal_corpus() -> Vec<(&'static str, Value)> {
    let valid = json!({
        "schema": "runx.signal.v1",
        "signal_id": "sig_1",
        "source_ref": a_ref(),
        "signal_type": "issue_opened",
        "title": "an issue opened",
        "observed_at": "2026-01-01T00:00:00Z",
    });
    let full = json!({
        "schema": "runx.signal.v1",
        "signal_id": "sig_1",
        "source_ref": a_ref(),
        "authenticity": {
            "host_ref": a_ref(),
            "principal_ref": a_ref(),
            "verified_by_ref": a_ref(),
            "trust_level": "verified_signature",
            "verified_at": "2026-01-01T00:00:00Z",
            "signature_refs": [a_ref()],
            "evidence_refs": [a_ref()],
        },
        "signal_type": "alert",
        "title": "an alert",
        "body_preview": "some body",
        "observed_at": "2026-01-01T00:00:00Z",
        "evidence_refs": [a_ref()],
        "fingerprint": {
            "algorithm": "sha256",
            "canonicalization": "json-c14n",
            "value": "abc",
            "derived_from": [a_ref()],
        },
        "extensions": { "k": 1 },
    });
    vec![
        ("minimal valid", valid.clone()),
        ("full valid", full),
        ("missing schema", {
            let mut v = valid.clone();
            remove_field(&mut v, "schema");
            v
        }),
        ("missing signal_id", {
            let mut v = valid.clone();
            remove_field(&mut v, "signal_id");
            v
        }),
        ("missing signal_type", {
            let mut v = valid.clone();
            remove_field(&mut v, "signal_type");
            v
        }),
        ("missing title", {
            let mut v = valid.clone();
            remove_field(&mut v, "title");
            v
        }),
        ("empty signal_id", {
            let mut v = valid.clone();
            v["signal_id"] = json!("");
            v
        }),
        ("empty title", {
            let mut v = valid.clone();
            v["title"] = json!("");
            v
        }),
        ("adapter-provided signal_type", {
            let mut v = valid.clone();
            v["signal_type"] = json!("adapter_specific_event");
            v
        }),
        ("empty signal_type rejected", {
            let mut v = valid.clone();
            v["signal_type"] = json!("");
            v
        }),
        ("malformed observed_at", {
            let mut v = valid.clone();
            v["observed_at"] = json!("not-a-timestamp");
            v
        }),
        ("additional property", {
            let mut v = valid.clone();
            v["bogus"] = json!(true);
            v
        }),
    ]
}

pub(super) fn source_packet_corpus() -> Vec<(&'static str, Value)> {
    let valid = json!({
        "schema": "runx.source_packet.v1",
        "packet_id": "src_pkt_001",
        "source_ref": a_ref(),
        "signal_type": "chat_message",
        "title": "Incoming customer message",
        "observed_at": "2026-05-28T12:00:00Z",
        "redaction_status": "redacted",
    });
    let full = json!({
        "schema": "runx.source_packet.v1",
        "packet_id": "src_pkt_full",
        "source_ref": a_ref(),
        "authenticity": {
            "host_ref": a_ref(),
            "trust_level": "verified_signature",
        },
        "signal_type": "alert",
        "title": "Production alert observed",
        "observed_at": "2026-05-28T12:05:00Z",
        "body_preview": "Latency spike on /charges",
        "redaction_status": "redacted",
        "workflow_inputs": { "severity": "high", "service": "charges" },
        "adapter_payload": { "raw_id": "1234", "channel": "ops" },
        "fingerprint": {
            "algorithm": "sha256",
            "canonicalization": "json-c14n",
            "value": "abc",
            "derived_from": [a_ref()],
        },
        "related_refs": [a_ref()],
        "extensions": { "trace": "t-1" },
    });
    vec![
        ("minimal valid", valid.clone()),
        ("full valid", full),
        ("adapter-provided signal_type", {
            let mut v = valid.clone();
            v["signal_type"] = json!("adapter_specific_event");
            v
        }),
        ("missing schema", {
            let mut v = valid.clone();
            remove_field(&mut v, "schema");
            v
        }),
        ("missing packet_id", {
            let mut v = valid.clone();
            remove_field(&mut v, "packet_id");
            v
        }),
        ("missing source_ref", {
            let mut v = valid.clone();
            remove_field(&mut v, "source_ref");
            v
        }),
        ("missing signal_type", {
            let mut v = valid.clone();
            remove_field(&mut v, "signal_type");
            v
        }),
        ("missing title", {
            let mut v = valid.clone();
            remove_field(&mut v, "title");
            v
        }),
        ("missing observed_at", {
            let mut v = valid.clone();
            remove_field(&mut v, "observed_at");
            v
        }),
        ("missing redaction_status", {
            let mut v = valid.clone();
            remove_field(&mut v, "redaction_status");
            v
        }),
        ("empty packet_id rejected", {
            let mut v = valid.clone();
            v["packet_id"] = json!("");
            v
        }),
        ("empty signal_type rejected", {
            let mut v = valid.clone();
            v["signal_type"] = json!("");
            v
        }),
        ("empty title rejected", {
            let mut v = valid.clone();
            v["title"] = json!("");
            v
        }),
        ("unknown redaction_status", {
            let mut v = valid.clone();
            v["redaction_status"] = json!("not_a_status");
            v
        }),
        ("malformed observed_at", {
            let mut v = valid.clone();
            v["observed_at"] = json!("not-a-timestamp");
            v
        }),
        ("additional property", {
            let mut v = valid.clone();
            v["bogus"] = json!(true);
            v
        }),
    ]
}

fn a_ref() -> Value {
    json!({ "type": "act", "uri": "runx:act:1" })
}

fn hash_commitment() -> Value {
    json!({ "algorithm": "sha256", "value": "abc", "canonicalization": "json-c14n" })
}

pub(super) fn doctor_corpus() -> Vec<(&'static str, Value)> {
    let summary = json!({ "errors": 0, "warnings": 0, "infos": 0 });
    vec![
        (
            "minimal valid",
            json!({
                "schema": "runx.doctor.v1",
                "status": "success",
                "summary": summary,
                "diagnostics": [],
            }),
        ),
        (
            "full valid",
            json!({
                "schema": "runx.doctor.v1",
                "status": "failure",
                "summary": summary,
                "diagnostics": [{
                    "id": "d1",
                    "instance_id": "i1",
                    "severity": "warning",
                    "title": "t",
                    "message": "m",
                    "target": {},
                    "location": { "path": "p", "json_pointer": "/a" },
                    "evidence": { "e": 1 },
                    "repairs": [{
                        "id": "r1",
                        "kind": "edit_json",
                        "confidence": "high",
                        "risk": "low",
                        "path": "p",
                        "requires_human_review": false,
                    }],
                }],
            }),
        ),
        (
            "missing status",
            json!({ "schema": "runx.doctor.v1", "summary": summary, "diagnostics": [] }),
        ),
        (
            "missing summary",
            json!({ "schema": "runx.doctor.v1", "status": "success", "diagnostics": [] }),
        ),
        (
            "missing schema",
            json!({ "status": "success", "summary": summary, "diagnostics": [] }),
        ),
        (
            "unknown status variant",
            json!({
                "schema": "runx.doctor.v1",
                "status": "maybe",
                "summary": summary,
                "diagnostics": [],
            }),
        ),
        (
            "additional property",
            json!({
                "schema": "runx.doctor.v1",
                "status": "success",
                "summary": summary,
                "diagnostics": [],
                "bogus": true,
            }),
        ),
        (
            "diagnostic missing required field",
            json!({
                "schema": "runx.doctor.v1",
                "status": "failure",
                "summary": summary,
                "diagnostics": [{
                    "id": "d1",
                    "severity": "error",
                    "title": "t",
                    "message": "m",
                    "target": {},
                    "location": { "path": "p" },
                    "repairs": [],
                }],
            }),
        ),
        ("not an object", json!("nope")),
    ]
}

pub(super) fn redaction_corpus() -> Vec<(&'static str, Value)> {
    let valid = json!({
        "schema": "runx.redaction.v1",
        "redaction_id": "red_1",
        "policy_ref": a_ref(),
        "redacted_fields": ["a", "b"],
        "hash_commitments": [hash_commitment()],
        "canonicalization": "json-c14n",
        "performed_by_ref": a_ref(),
        "performed_at": "2026-01-01T00:00:00Z",
    });
    vec![
        ("full valid", valid.clone()),
        (
            "minimal valid",
            json!({
                "schema": "runx.redaction.v1",
                "redaction_id": "red_1",
                "policy_ref": a_ref(),
                "redacted_fields": [],
                "hash_commitments": [],
                "canonicalization": "json-c14n",
                "performed_by_ref": a_ref(),
                "performed_at": "2026-01-01T00:00:00Z",
            }),
        ),
        ("missing schema", {
            let mut v = valid.clone();
            remove_field(&mut v, "schema");
            v
        }),
        ("missing redaction_id", {
            let mut v = valid.clone();
            remove_field(&mut v, "redaction_id");
            v
        }),
        ("empty redaction_id", {
            let mut v = valid.clone();
            v["redaction_id"] = json!("");
            v
        }),
        ("empty canonicalization", {
            let mut v = valid.clone();
            v["canonicalization"] = json!("");
            v
        }),
        ("empty redacted_fields item", {
            let mut v = valid.clone();
            v["redacted_fields"] = json!([""]);
            v
        }),
        ("malformed performed_at", {
            let mut v = valid.clone();
            v["performed_at"] = json!("not-a-timestamp");
            v
        }),
        ("additional property", {
            let mut v = valid.clone();
            v["bogus"] = json!(true);
            v
        }),
    ]
}

pub(super) fn artifact_corpus() -> Vec<(&'static str, Value)> {
    let valid = json!({
        "schema": "runx.artifact.v1",
        "artifact_id": "art_1",
        "artifact_ref": a_ref(),
        "produced_by": { "receipt_ref": a_ref() },
        "media_type": "text/plain",
        "created_at": "2026-01-01T00:00:00Z",
        "size_bytes": 12,
        "hash": hash_commitment(),
        "redaction_refs": [],
        "source_refs": [],
    });
    vec![
        ("minimal valid", valid.clone()),
        ("full valid", {
            let mut v = valid.clone();
            v["produced_by"] = json!({
                "receipt_ref": a_ref(),
                "act_ref": { "receipt_ref": a_ref(), "act_id": "act_1" },
            });
            v["data_ref"] = a_ref();
            v["summary"] = json!("a summary");
            v["extensions"] = json!({ "k": 1 });
            v
        }),
        ("missing schema", {
            let mut v = valid.clone();
            remove_field(&mut v, "schema");
            v
        }),
        ("missing artifact_id", {
            let mut v = valid.clone();
            remove_field(&mut v, "artifact_id");
            v
        }),
        ("missing hash", {
            let mut v = valid.clone();
            remove_field(&mut v, "hash");
            v
        }),
        ("empty artifact_id", {
            let mut v = valid.clone();
            v["artifact_id"] = json!("");
            v
        }),
        ("empty media_type", {
            let mut v = valid.clone();
            v["media_type"] = json!("");
            v
        }),
        ("malformed created_at", {
            let mut v = valid.clone();
            v["created_at"] = json!("nope");
            v
        }),
        ("empty hash value", {
            let mut v = valid.clone();
            v["hash"] = json!({ "algorithm": "sha256", "value": "", "canonicalization": "c" });
            v
        }),
        ("additional property", {
            let mut v = valid.clone();
            v["bogus"] = json!(true);
            v
        }),
    ]
}

pub(super) fn reference_corpus() -> Vec<(&'static str, Value)> {
    vec![
        (
            "minimal valid",
            json!({ "type": "tracking_item", "uri": "runx:tracking_item:1" }),
        ),
        (
            "full valid",
            json!({
                "type": "act",
                "uri": "runx:act:1",
                "provider": "github",
                "locator": "owner/repo#1",
                "label": "an act",
                "observed_at": "2026-01-01T00:00:00.000Z",
                "proof_kind": "effect_evidence",
            }),
        ),
        (
            "optional schema marker",
            json!({ "schema": "runx.reference.v1", "type": "act", "uri": "x" }),
        ),
        ("missing uri", json!({ "type": "act" })),
        ("missing type", json!({ "uri": "x" })),
        (
            "unknown type variant",
            json!({ "type": "not_a_type", "uri": "x" }),
        ),
        ("empty uri", json!({ "type": "act", "uri": "" })),
        (
            "malformed observed_at",
            json!({ "type": "act", "uri": "x", "observed_at": "not-a-timestamp" }),
        ),
        (
            "additional property",
            json!({ "type": "act", "uri": "x", "bogus": true }),
        ),
        (
            "bad proof_kind",
            json!({ "type": "act", "uri": "x", "proof_kind": "wire" }),
        ),
    ]
}

pub(super) fn reference_link_corpus() -> Vec<(&'static str, Value)> {
    let valid = json!({
        "role": "source_thread",
        "ref": {
            "type": "thread",
            "uri": "provider://workspace/channel/thread",
            "provider": "example",
            "locator": "workspace/channel/thread",
            "label": "source thread"
        }
    });

    vec![
        ("valid", valid.clone()),
        ("missing role", {
            let mut v = valid.clone();
            remove_field(&mut v, "role");
            v
        }),
        ("missing ref", {
            let mut v = valid.clone();
            remove_field(&mut v, "ref");
            v
        }),
        ("empty role", {
            let mut v = valid.clone();
            v["role"] = json!("");
            v
        }),
        ("additional property", {
            let mut v = valid.clone();
            v["bogus"] = json!(true);
            v
        }),
    ]
}
