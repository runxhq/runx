//! Financial authority binds the provider's exact inputs, never a caller's
//! unrelated budget declaration. Unpriced operations keep their existing wire
//! value and digest; a money-moving operation declares both source expressions.
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use serde_json::{Value, json};

use super::{Template, TemplateInputs};
use crate::schema::{NonEmptyString, RunxSchema};
use crate::{JsonValue, MAX_PORTABLE_INTEGER, OperationAmount};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum AmountRule {
    #[default]
    None,
    Bound(AmountBinding),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct AmountBinding {
    /// Exact integer smallest units sent to the provider, or the exact
    /// bound maximum a protocol's settlement operation is allowed to spend.
    pub units: Template,
    /// Provider currency/asset code; financial authority uses its canonical
    /// uppercase code, so a provider's lowercase `usd` still means USD cents.
    pub currency: Template,
}

impl AmountRule {
    #[must_use]
    pub fn requires_amount(&self) -> bool {
        matches!(self, Self::Bound(_))
    }

    pub fn resolve(&self, arguments: &JsonValue) -> Result<Option<OperationAmount>, String> {
        let Self::Bound(binding) = self else {
            return Ok(None);
        };
        let inputs = TemplateInputs {
            arguments: Some(arguments),
            ..TemplateInputs::default()
        };
        let units = binding
            .units
            .render(&inputs)
            .map_err(|_| "the operation's amount units cannot be resolved")?
            .as_u64()
            .filter(|value| *value <= MAX_PORTABLE_INTEGER)
            .ok_or("the operation's amount units must be a portable nonnegative integer")?;
        let currency = binding
            .currency
            .render_string(&inputs)
            .map_err(|_| "the operation's amount currency cannot be resolved")?
            .to_ascii_uppercase();
        if !(3..=16).contains(&currency.len())
            || !currency.starts_with(|c: char| c.is_ascii_uppercase())
            || !currency
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
        {
            return Err(
                "the operation's amount currency must be an ASCII currency or asset code"
                    .to_owned(),
            );
        }
        let unit =
            NonEmptyString::new(currency).ok_or("the operation's amount currency is empty")?;
        Ok(Some(OperationAmount { units, unit }))
    }
}

impl Serialize for AmountRule {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::None => serializer.serialize_str("none"),
            Self::Bound(binding) => binding.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for AmountRule {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // `required` cannot bind an approved amount to a provider request and
        // is deliberately refused. There is one financial declaration shape.
        let value = JsonValue::deserialize(deserializer)?;
        match value {
            JsonValue::String(value) if value == "none" => Ok(Self::None),
            JsonValue::Object(_) => value
                .deserialize_into()
                .map(Self::Bound)
                .map_err(de::Error::custom),
            _ => Err(de::Error::custom(
                "amount is none or an object binding units and currency to provider inputs",
            )),
        }
    }
}

impl RunxSchema for AmountRule {
    fn json_schema() -> Value {
        json!({"oneOf": [{"const":"none"}, AmountBinding::json_schema()]})
    }
}
