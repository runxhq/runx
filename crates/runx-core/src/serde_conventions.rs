//! Serde conventions for the kernel.
//!
//! Public structs serialize with camelCase field names. Tagged unions use one
//! discriminator field, usually `type` for state-machine events and plans. Optional fields are
//! omitted when absent. Serialized maps use deterministic key order.

#[cfg(test)]
mod tests {
    use crate::state_machine::SequentialGraphPlan;

    #[test]
    fn state_machine_plan_uses_type_tag_and_camel_case_fields() -> Result<(), serde_json::Error> {
        let plan = SequentialGraphPlan::RunStep {
            step_id: "review".to_owned(),
            attempt: 1,
            context_from: vec!["read".to_owned()],
        };

        let json = serde_json::to_string(&plan)?;

        assert_eq!(
            json,
            r#"{"type":"run_step","stepId":"review","attempt":1,"contextFrom":["read"]}"#,
        );
        Ok(())
    }
}
