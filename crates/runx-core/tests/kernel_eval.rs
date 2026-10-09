use runx_contracts::JsonValue;
use runx_core::kernel_eval::{KernelEvalError, KernelEvalOutput, evaluate_kernel_document_str};

#[test]
fn evaluates_state_machine_fixture_document() -> Result<(), Box<dyn std::error::Error>> {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../fixtures/kernel/state-machine/sequential-plan-first-step.json"
    ))?;
    let output = evaluate_kernel_document_str(&fixture["input"].to_string())?;

    let KernelEvalOutput::Output { value } = output;
    assert_eq!(
        value,
        json_value(
            r#"{
            "type": "run_step",
            "stepId": "first",
            "attempt": 1,
            "contextFrom": []
        }"#
        )?
    );
    Ok(())
}

#[test]
fn evaluates_raw_input_document() -> Result<(), Box<dyn std::error::Error>> {
    let output = evaluate_kernel_document_str(
        r#"{"kind":"policy.scopeGrantAllows","grantedScope":"repo:*","requestedScope":"repo:read","policy":"delegated"}"#,
    )?;

    let KernelEvalOutput::Output { value } = output;
    assert_eq!(value, json_value("true")?);
    Ok(())
}

#[test]
fn rejects_oversized_documents_fail_closed() {
    let source = format!(
        r#"{{"kind":"state-machine.createSequentialGraphState","graphId":"{}","steps":[]}}"#,
        "a".repeat(1024 * 1024)
    );

    assert_invalid_input_contains(&source, "exceeds 1048576 bytes");
}

#[test]
fn rejects_deeply_nested_documents_fail_closed() {
    let source = format!(
        r#"{{"kind":"state-machine.planSequentialGraphTransition","decision":{}}}"#,
        nested_json_object(65)
    );

    assert_invalid_input_contains(&source, "exceeds JSON depth 64");
}

#[test]
fn rejects_wide_documents_fail_closed() {
    let fields = (0..513)
        .map(|index| format!(r#""k{index}":null"#))
        .collect::<Vec<_>>()
        .join(",");
    let source = format!(
        r#"{{"kind":"state-machine.planSequentialGraphTransition","decision":{{{fields}}}}}"#
    );

    assert_invalid_input_contains(&source, "object exceeds 512 fields");
}

fn json_value(source: &str) -> Result<JsonValue, serde_json::Error> {
    serde_json::from_str(source)
}

fn nested_json_object(depth: usize) -> String {
    let mut source = String::from(r#"{"leaf":"value"}"#);
    for _ in 0..depth {
        source = format!(r#"{{"child":{source}}}"#);
    }
    source
}

fn assert_invalid_input_contains(source: &str, expected_message: &str) {
    let result = evaluate_kernel_document_str(source);

    match result {
        Err(KernelEvalError::InvalidInput(message)) => {
            assert!(message.contains(expected_message), "{message}");
        }
        other => {
            assert_eq!(
                other.map(|output| output_kind(&output)),
                Err(KernelEvalError::InvalidInput(expected_message.to_owned()))
            );
        }
    }
}

fn output_kind(output: &KernelEvalOutput) -> &'static str {
    match output {
        KernelEvalOutput::Output { .. } => "output",
    }
}
