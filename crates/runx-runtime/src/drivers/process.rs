//! The `process` driver: an operation performed by a declared tool package,
//! a trusted host process that receives the operation's credential.
//!
//! A read runs the tool with the call's input. A write runs it with the input,
//! `phase: "perform"` and runx's provider key as `idempotency_key`; its output
//! is the acknowledgement. The write is
//! confirmed by running the tool again with `phase: "readback"` and the
//! `acknowledgement`, and the kernel matches what it observes. A tool that
//! prints `{"refused": "<reason>"}` refused to act; any other failure may
//! have acted.

use runx_contracts::connector::{ProcessImplementation, stable_digest};
use runx_contracts::{JsonObject, JsonValue};

use crate::credentials::CredentialDelivery;
use crate::kernel::operation::confirm::{Check, Driver, Observed, Unperformed};
use crate::kernel::operation::{AdmittedCall, ToolRunner};

pub(crate) struct Process<'a> {
    pub tools: &'a dyn ToolRunner,
    pub call: &'a AdmittedCall<'a>,
    pub spec: &'a ProcessImplementation,
    pub scopes: &'a [String],
    pub delivery: &'a CredentialDelivery,
}

impl Driver for Process<'_> {
    fn perform(&self, _item: Option<usize>) -> Result<Observed, Unperformed> {
        let answer = self
            .run("perform", None)
            .map_err(|reason| Unperformed::MaybeSent {
                code: "tool_failed",
                reason,
            })?;
        if let Some(reason) = refusal(&answer) {
            return Err(Unperformed::Refused {
                code: "tool_refused",
                reason,
                answer: None,
            });
        }
        observed(answer).map_err(|reason| Unperformed::MaybeSent {
            code: "invalid_response",
            reason,
        })
    }

    fn check(&self, check: Check, write: &JsonValue) -> Result<Observed, String> {
        let Check::Readback {
            readback: 0,
            item: None,
        } = check
        else {
            return Err(format!("the implementation declares no {check:?}"));
        };
        observed(self.run("readback", Some(write))?)
    }

    fn result(
        &self,
        write: &JsonValue,
        _confirmed: &[JsonValue],
    ) -> Result<(JsonValue, Option<String>), String> {
        // A write is identified by the key it was sent with, which is stable
        // for its intent. Its answer cannot identify it: two different writes
        // may be answered alike.
        let operation_id = match self.call.mutation() {
            true => Some(format!(
                "process:{}",
                self.call
                    .idempotency_key
                    .ok_or("a write is sent with an idempotency key")?
            )),
            false => None,
        };
        Ok((write.clone(), operation_id))
    }
}

impl Process<'_> {
    /// Run the tool for `phase`; a read's tool sees only the call's input.
    fn run(&self, phase: &str, acknowledgement: Option<&JsonValue>) -> Result<JsonValue, String> {
        let mut inputs = match self.call.arguments {
            JsonValue::Object(arguments) => arguments.clone(),
            _ => JsonObject::new(),
        };
        if self.call.mutation() {
            inputs.insert("phase".to_owned(), JsonValue::String(phase.to_owned()));
            if let Some(key) = self.call.idempotency_key {
                inputs.insert(
                    "idempotency_key".to_owned(),
                    JsonValue::String(key.to_owned()),
                );
            }
            if let Some(acknowledgement) = acknowledgement {
                inputs.insert("acknowledgement".to_owned(), acknowledgement.clone());
            }
        }
        self.tools
            .run(self.spec.tool.as_str(), inputs, self.scopes, self.delivery)
    }
}

fn refusal(answer: &JsonValue) -> Option<String> {
    match answer {
        JsonValue::Object(object) if object.len() == 1 => object
            .get("refused")
            .and_then(JsonValue::as_str)
            .map(str::to_owned),
        _ => None,
    }
}

fn observed(value: JsonValue) -> Result<Observed, String> {
    let evidence = stable_digest(&value).map_err(|error| error.to_string())?;
    Ok(Observed {
        value,
        evidence: vec![evidence.as_str().to_owned()],
    })
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    use runx_contracts::connector::OperationAccess;

    use super::*;

    /// Records what it was run with; refuses `blocked` branches and fails
    /// on `broken` ones.
    struct Tool {
        runs: RefCell<Vec<JsonObject>>,
    }

    impl ToolRunner for Tool {
        fn run(
            &self,
            _tool: &str,
            inputs: JsonObject,
            _scopes: &[String],
            _delivery: &CredentialDelivery,
        ) -> Result<JsonValue, String> {
            self.runs.borrow_mut().push(inputs.clone());
            match inputs.get("branch").and_then(JsonValue::as_str) {
                Some("blocked") => Ok(serde_json::from_value(
                    serde_json::json!({"refused": "protected"}),
                )
                .map_err(|error| error.to_string())?),
                Some("broken") => Err("the tool exited 1".to_owned()),
                _ => Ok(serde_json::from_value(serde_json::json!({"sha": "abc123"}))
                    .map_err(|error| error.to_string())?),
            }
        }
    }

    fn spec() -> ProcessImplementation {
        ProcessImplementation {
            tool: "git.push".to_owned().into(),
            runtime_scopes: Vec::new(),
            readback: None,
        }
    }

    fn arguments(branch: &str) -> JsonValue {
        JsonValue::Object(JsonObject::from([(
            "branch".to_owned(),
            JsonValue::String(branch.to_owned()),
        )]))
    }

    #[test]
    fn a_tool_performs_reads_back_and_refuses_by_the_protocol() -> Result<(), String> {
        let tool = Tool {
            runs: RefCell::new(Vec::new()),
        };
        let (spec, parameters, env, delivery) = (
            spec(),
            JsonObject::new(),
            BTreeMap::new(),
            CredentialDelivery::none(),
        );
        let write = |branch: &str| {
            let arguments = arguments(branch);
            let call =
                AdmittedCall::for_tests(&arguments, &parameters, &env, OperationAccess::Mutate);
            let driver = Process {
                tools: &tool,
                call: &call,
                spec: &spec,
                scopes: &[],
                delivery: &delivery,
            };
            let performed = driver.perform(None);
            let read = performed.as_ref().ok().map(|observed| {
                driver.check(
                    Check::Readback {
                        readback: 0,
                        item: None,
                    },
                    &observed.value,
                )
            });
            (performed, read)
        };

        let (performed, read) = write("feature");
        assert!(matches!(read, Some(Ok(_))));
        // The write is identified by the key it was sent with, not by its
        // answer: another write answered alike is another operation.
        let feature = arguments("feature");
        let call = AdmittedCall::for_tests(&feature, &parameters, &env, OperationAccess::Mutate);
        let driver = Process {
            tools: &tool,
            call: &call,
            spec: &spec,
            scopes: &[],
            delivery: &delivery,
        };
        let answer = performed.map_err(Unperformed::reason)?.value;
        let (_, operation_id) = driver.result(&answer, &[])?;
        assert_eq!(operation_id.as_deref(), Some("process:key-1"));
        let runs = tool.runs.borrow().clone();
        assert_eq!(
            runs[0].get("phase"),
            Some(&JsonValue::String("perform".to_owned()))
        );
        assert_eq!(
            runs[1].get("phase"),
            Some(&JsonValue::String("readback".to_owned()))
        );
        assert!(runs[1].contains_key("acknowledgement"));

        assert!(matches!(
            write("blocked").0,
            Err(Unperformed::Refused {
                code: "tool_refused",
                ..
            })
        ));
        assert!(matches!(
            write("broken").0,
            Err(Unperformed::MaybeSent { .. })
        ));

        let arguments = arguments("feature");
        let call = AdmittedCall::for_tests(&arguments, &parameters, &env, OperationAccess::Read);
        let read = Process {
            tools: &tool,
            call: &call,
            spec: &spec,
            scopes: &[],
            delivery: &delivery,
        };
        read.perform(None).map_err(Unperformed::reason)?;
        assert!(
            !tool
                .runs
                .borrow()
                .last()
                .is_some_and(|inputs| inputs.contains_key("phase"))
        );
        Ok(())
    }
}
