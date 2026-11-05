//! What a provider charges for one call of an operation, and what a call was
//! charged. Money is integer micro-USD (1 = $0.000001): never a float, and
//! USD only until a provider bills in another currency.

use serde::{Deserialize, Serialize};

use crate::schema::{NonEmptyString, RunxSchema};

/// A provider's charge for one call. A missing USD quote is metered usage
/// whose monetary price is unknown; `usd_micro: 0` explicitly declares free.
/// No declaration at all means no charge is known, which is not a quote of
/// zero: a money limit admits only a known quote. The quote and basis are
/// authority-bound.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct OperationCost {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usd_micro: Option<u64>,
    pub per: CostBasis,
    /// Where the price was read.
    pub source: CostSource,
    /// The page that states it, `https` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<NonEmptyString>,
}

/// When a call becomes billable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum CostBasis {
    /// Once the request may have left.
    Request,
    /// Only when the provider returns an answer the operation accepts.
    Success,
}

impl CostBasis {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Request => "request",
            Self::Success => "success",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum CostSource {
    /// The provider's documentation.
    Docs,
    /// The provider's rate card API.
    RateCard,
    /// What the provider was seen to charge.
    Observed,
}

/// Who pays a priced call.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum CostPayer {
    /// The account the instance binds: the operator's own key.
    ProviderAccount,
}

/// What a sealed call cost, as its packet carries it. The quote is the
/// operation's declared charge; the settled amount is what the call was
/// charged, or `None` while that is unresolved.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct CallCost {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quoted_usd_micro: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settled_usd_micro: Option<u64>,
    pub payer: CostPayer,
}
