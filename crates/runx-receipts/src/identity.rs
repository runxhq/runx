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
    pub actors: Vec<String>,
}

#[must_use]
pub fn signed_display_identity(receipt: &Receipt) -> SignedDisplayIdentity {
    SignedDisplayIdentity {
        subject_kind: receipt.subject.kind.to_string(),
        subject_ref: receipt.subject.reference.uri.to_string(),
        issuer_type: issuer_type_name(&receipt.issuer.issuer_type).to_owned(),
        actors: signed_actor_labels(receipt),
    }
}

fn signed_actor_labels(receipt: &Receipt) -> Vec<String> {
    let mut actors = Vec::new();
    push_unique(&mut actors, receipt.authority.actor_ref.uri.to_string());
    for act in &receipt.acts {
        if let Some(by) = &act.by {
            if let Some(provider) = by.provider.as_deref() {
                push_unique(&mut actors, provider.to_owned());
            }
            if let Some(model) = by.model.as_deref() {
                push_unique(&mut actors, model.to_owned());
            }
        }
    }
    actors
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !value.is_empty() && !values.iter().any(|existing| existing == &value) {
        values.push(value);
    }
}

fn issuer_type_name(issuer_type: &ReceiptIssuerType) -> &'static str {
    match issuer_type {
        ReceiptIssuerType::Local => "local",
        ReceiptIssuerType::Hosted => "hosted",
        ReceiptIssuerType::Ci => "ci",
        ReceiptIssuerType::Verifier => "verifier",
    }
}
