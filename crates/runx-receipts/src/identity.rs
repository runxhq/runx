use runx_contracts::Receipt;

/// Display identity derived only from fields inside the signed receipt body.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SignedDisplayIdentity {
    pub subject_ref: String,
    pub issuer_type: String,
    pub actor: Option<String>,
}

#[must_use]
pub fn signed_display_identity(receipt: &Receipt) -> SignedDisplayIdentity {
    SignedDisplayIdentity {
        subject_ref: receipt.subject.reference.uri.to_string(),
        issuer_type: receipt.issuer.issuer_type.label().to_owned(),
        actor: receipt
            .authority
            .actor_ref
            .as_ref()
            .map(|actor| actor.uri.to_string()),
    }
}
