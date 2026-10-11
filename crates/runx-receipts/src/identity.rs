use runx_contracts::{Receipt, ReceiptIssuerType};

/// Display identity derived only from fields inside the signed receipt body.
///
/// `Receipt.metadata` is a runtime-local read aid. It must never supply
/// trust-bearing identity labels because it is stripped from the signed body.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SignedDisplayIdentity {
    pub subject_kind: String,
    pub subject_ref: String,
    pub issuer_type: String,
    pub actor: String,
}

#[must_use]
pub fn signed_display_identity(receipt: &Receipt) -> SignedDisplayIdentity {
    SignedDisplayIdentity {
        subject_kind: receipt.subject.kind.to_string(),
        subject_ref: receipt.subject.reference.uri.to_string(),
        issuer_type: issuer_type_name(&receipt.issuer.issuer_type).to_owned(),
        actor: receipt.authority.actor_ref.uri.to_string(),
    }
}

fn issuer_type_name(issuer_type: &ReceiptIssuerType) -> &'static str {
    match issuer_type {
        ReceiptIssuerType::Local => "local",
        ReceiptIssuerType::Hosted => "hosted",
        ReceiptIssuerType::Ci => "ci",
    }
}
