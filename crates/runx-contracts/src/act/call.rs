//! The act a governed call performs, as whoever makes the call states it.
//!
//! A call's act says what kind of act it is and why it is made. Who made it,
//! under what authority and why that was legitimate come from admission, not
//! from here; what it produced comes from the operation.

use serde::{Deserialize, Serialize};

use crate::connector::Digest;
use crate::schema::{NonEmptyString, RunxSchema};
use crate::{ActForm, DecisionChoice};

/// The longest purpose or reason a call's act may state.
pub const CALL_ACT_TEXT_MAX: usize = 512;
/// The longest target locator a call's act may name.
pub const CALL_ACT_TARGET_MAX: usize = 2048;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct CallAct {
    /// What kind of act the call is. Revision and verification acts carry
    /// details a single call does not have, so a call is one of these.
    #[serde(default, skip_serializing_if = "CallActForm::is_observation")]
    pub form: CallActForm,
    /// Why the call is made.
    pub purpose: NonEmptyString,
    /// What a reader is told about this particular call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<NonEmptyString>,
    /// The locator of what the act is about.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<NonEmptyString>,
    /// The decision the call carries out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<CallDecision>,
    /// The receipt of the act this one follows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous: Option<Digest>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum CallActForm {
    #[default]
    Observation,
    Reply,
    Review,
}

impl CallActForm {
    fn is_observation(&self) -> bool {
        *self == Self::Observation
    }
}

impl From<CallActForm> for ActForm {
    fn from(form: CallActForm) -> Self {
        match form {
            CallActForm::Observation => Self::Observation,
            CallActForm::Reply => Self::Reply,
            CallActForm::Review => Self::Review,
        }
    }
}

/// The decisions a call can carry out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum CallDecision {
    Continue,
    Close,
    Decline,
    Defer,
    Escalate,
    Monitor,
}

impl From<CallDecision> for DecisionChoice {
    fn from(decision: CallDecision) -> Self {
        match decision {
            CallDecision::Continue => Self::Continue,
            CallDecision::Close => Self::Close,
            CallDecision::Decline => Self::Decline,
            CallDecision::Defer => Self::Defer,
            CallDecision::Escalate => Self::Escalate,
            CallDecision::Monitor => Self::Monitor,
        }
    }
}

impl CallAct {
    /// The fields an act states.
    pub const FIELDS: &'static [&'static str] = &[
        "form", "purpose", "reason", "target", "decision", "previous",
    ];

    /// Read an act from its JSON form, refusing anything outside its shape.
    pub fn parse(value: &crate::JsonValue) -> Result<Self, String> {
        let act: Self = value
            .clone()
            .deserialize_into()
            .map_err(|error| format!("act is invalid: {error}"))?;
        act.check()?;
        Ok(act)
    }

    /// The bounds serde does not express: text a person reads is short and
    /// printable, and a target is one locator.
    pub fn check(&self) -> Result<(), String> {
        text(self.purpose.as_str(), "act.purpose")?;
        if let Some(reason) = &self.reason {
            text(reason.as_str(), "act.reason")?;
        }
        if let Some(target) = &self.target {
            let target = target.as_str();
            if target.len() > CALL_ACT_TARGET_MAX
                || target
                    .chars()
                    .any(|ch| ch.is_whitespace() || ch.is_control())
            {
                return Err(format!(
                    "act.target must be one locator of at most {CALL_ACT_TARGET_MAX} bytes"
                ));
            }
        }
        Ok(())
    }

    /// What a reader is told: the reason, or the purpose when no reason is
    /// given.
    #[must_use]
    pub fn summary(&self) -> &NonEmptyString {
        self.reason.as_ref().unwrap_or(&self.purpose)
    }
}

fn text(value: &str, field: &str) -> Result<(), String> {
    if value.trim().is_empty()
        || value.len() > CALL_ACT_TEXT_MAX
        || value.chars().any(char::is_control)
    {
        Err(format!(
            "{field} must be printable text of at most {CALL_ACT_TEXT_MAX} bytes"
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(value: serde_json::Value) -> Result<CallAct, String> {
        CallAct::parse(
            &crate::JsonValue::from_serialize(&value).map_err(|error| error.to_string())?,
        )
    }

    #[test]
    fn an_act_needs_a_purpose_and_reads_only_its_own_fields() -> Result<(), String> {
        let act = parse(serde_json::json!({"purpose": "Close the request"}))?;
        assert_eq!(act.form, CallActForm::Observation);
        assert_eq!(act.summary().as_str(), "Close the request");
        assert!(parse(serde_json::json!({"reason": "no purpose"})).is_err());
        assert!(parse(serde_json::json!({"purpose": "x", "legitimacy": "y"})).is_err());
        Ok(())
    }

    #[test]
    fn fields_name_every_field_an_act_states() -> Result<(), String> {
        let act = parse(serde_json::json!({
            "form": "reply",
            "purpose": "x",
            "reason": "y",
            "target": "github://a/b/issues/1",
            "decision": "close",
            "previous": format!("sha256:{}", "a".repeat(64)),
        }))?;
        let serde_json::Value::Object(stated) =
            serde_json::to_value(&act).map_err(|error| error.to_string())?
        else {
            return Err("an act serializes as an object".to_owned());
        };
        let mut stated = stated.keys().map(String::as_str).collect::<Vec<_>>();
        let mut fields = CallAct::FIELDS.to_vec();
        stated.sort_unstable();
        fields.sort_unstable();
        assert_eq!(stated, fields);
        Ok(())
    }

    #[test]
    fn form_and_decision_are_closed() {
        assert!(
            parse(serde_json::json!({"purpose": "x", "form": "review", "decision": "decline"}))
                .is_ok()
        );
        assert!(parse(serde_json::json!({"purpose": "x", "form": "revision"})).is_err());
        assert!(parse(serde_json::json!({"purpose": "x", "form": "judgment"})).is_err());
        assert!(parse(serde_json::json!({"purpose": "x", "decision": "paid"})).is_err());
        assert!(parse(serde_json::json!({"purpose": "x", "decision": "open"})).is_err());
    }

    #[test]
    fn previous_is_a_receipt_id_and_target_one_locator() {
        let receipt = format!("sha256:{}", "a".repeat(64));
        assert!(parse(serde_json::json!({"purpose": "x", "previous": receipt})).is_ok());
        assert!(parse(serde_json::json!({"purpose": "x", "previous": "rcpt_1"})).is_err());
        assert!(
            parse(serde_json::json!({"purpose": "x", "target": "github://a/b/issues/1"})).is_ok()
        );
        assert!(parse(serde_json::json!({"purpose": "x", "target": "two words"})).is_err());
        assert!(parse(serde_json::json!({"purpose": "x".repeat(513)})).is_err());
    }
}
