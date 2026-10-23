//! Packet ids. A packet is a named, versioned JSON Schema a step's output or a
//! runner's input is checked against; its id says who owns it, and so where
//! it resolves from.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, de};
use serde_json::{Value, json};

use crate::connector::ids::identifier;
use crate::connector::is_publisher_namespace;
use crate::schema::{RunxSchema, SchemaMapKey, schema_id_url};

/// `runx.<name>.v<N>` for a core packet, which the kernel emits or reads and
/// the binary carries; `<publisher>/<package>.<name>.v<N>` for a package's,
/// the package id followed by the packet's name. A name is dotted snake_case
/// segments; a version counts from 1.
pub const PACKET_ID_PATTERN: &str = r"^(runx|[a-z0-9](?:[a-z0-9-]{0,62}[a-z0-9])?/[a-z0-9]+(?:-[a-z0-9]+)*)(\.[a-z][a-z0-9_]*)+\.v[1-9][0-9]*$";
pub const PACKET_ID_MAX_LENGTH: usize = 200;
/// What a refusal of a value outside [`PACKET_ID_PATTERN`] names.
pub const PACKET_ID_EXPECTED: &str = "a packet id is runx.<name>.v<N> for a core packet or <publisher>/<package>.<name>.v<N> for a package's";

identifier!(
    /// A packet's id, in one of the two forms of [`PACKET_ID_PATTERN`]. The
    /// forms never collide: a core id has no `/`, and no package name is
    /// reserved.
    PacketId,
    PACKET_ID_PATTERN,
    PACKET_ID_MAX_LENGTH,
    is_packet_id,
    "packet id"
);

struct PacketIdParts<'a> {
    /// `<publisher>` and `<package>`; `None` for a core packet.
    owner: Option<(&'a str, &'a str)>,
    name: &'a str,
    version: &'a str,
}

#[must_use]
pub fn is_packet_id(value: &str) -> bool {
    value.len() <= PACKET_ID_MAX_LENGTH && packet_id_parts(value).is_some()
}

fn packet_id_parts(value: &str) -> Option<PacketIdParts<'_>> {
    let (head, version) = value.rsplit_once(".v")?;
    let mut digits = version.bytes();
    if !digits
        .next()
        .is_some_and(|byte| (b'1'..=b'9').contains(&byte))
        || !digits.all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let (owner, name) = match head.split_once('/') {
        Some((publisher, rest)) => {
            let (package, name) = rest.split_once('.')?;
            if !is_publisher_namespace(publisher) || !is_package_name(package) {
                return None;
            }
            (Some((publisher, package)), name)
        }
        None => (None, head.strip_prefix("runx.")?),
    };
    name.split('.')
        .all(is_name_segment)
        .then_some(PacketIdParts {
            owner,
            name,
            version,
        })
}

/// A skill package's name: lowercase letters and digits in hyphen-separated
/// runs, at most 64 characters.
fn is_package_name(value: &str) -> bool {
    value.len() <= 64
        && value.split('-').all(|run| {
            !run.is_empty()
                && run
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        })
}

fn is_name_segment(segment: &str) -> bool {
    let mut bytes = segment.bytes();
    bytes.next().is_some_and(|byte| byte.is_ascii_lowercase())
        && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

impl PacketId {
    /// A core packet id this crate declares as a literal; the native packet
    /// tests check each against its contract's schema.
    pub(crate) fn core(id: &'static str) -> Self {
        debug_assert!(is_packet_id(id) && !id.contains('/'), "{id}");
        Self(id.to_owned())
    }

    fn parts(&self) -> PacketIdParts<'_> {
        packet_id_parts(&self.0).unwrap_or(PacketIdParts {
            owner: None,
            name: "",
            version: "",
        })
    }

    /// Whether the kernel owns this packet, which then resolves only from the
    /// contracts compiled into runx.
    #[must_use]
    pub fn is_core(&self) -> bool {
        self.parts().owner.is_none()
    }

    /// The `<publisher>` and `<package>` a package packet's id names; `None`
    /// for a core packet.
    #[must_use]
    pub fn owner(&self) -> Option<(&str, &str)> {
        self.parts().owner
    }

    /// `<name>.v<N>.schema.json`, the file a package packet is in its owner's
    /// `packets/`; `None` for a core packet.
    #[must_use]
    pub fn file_name(&self) -> Option<String> {
        let PacketIdParts {
            owner,
            name,
            version,
        } = self.parts();
        owner.map(|_| format!("{name}.v{version}.schema.json"))
    }

    /// `<publisher>/<package>/<name>.v<N>.schema.json`, where a package that
    /// reads another package's packet carries its pinned copy, under its own
    /// `packets/`; `None` for a core packet.
    #[must_use]
    pub fn pinned_path(&self) -> Option<String> {
        let (publisher, package) = self.owner()?;
        Some(format!("{publisher}/{package}/{}", self.file_name()?))
    }

    /// The packet's canonical schema URL, its `$id`, by [`schema_id_url`]: a
    /// core packet's like any core schema's, a package packet's under
    /// `packages/<publisher>/<package>/`.
    #[must_use]
    pub fn schema_url(&self) -> String {
        if self.is_core() {
            schema_id_url(&self.0)
        } else {
            schema_id_url(&format!("packages.{}", self.0))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{PacketId, is_packet_id};

    #[test]
    fn accepts_the_core_and_package_forms() {
        for id in [
            "runx.receipt.v1",
            "runx.payment.cancel_purchase.request.v1",
            "runx.vault.v12",
            "runx/twitter.plan.v1",
            "runx/github-issue-to-pr.host_result.v1",
            "acme-co/lead-qualify.qualification.v2",
            "0state/x.y.z.v1",
        ] {
            assert!(is_packet_id(id), "{id}");
        }
    }

    #[test]
    fn refuses_every_other_form() {
        for id in [
            "",
            "twitter.plan.v1",
            "runx.v1",
            "runx.plan",
            "runx.plan.v0",
            "runx.plan.v01",
            "runx.plan.v1.",
            "runx.Plan.v1",
            "runx.issue-triage.v1",
            "runx/twitter.v1",
            "runx/twitter",
            "runx/Twitter.plan.v1",
            "runx/twitter-.plan.v1",
            "-runx/twitter.plan.v1",
            "runx/twitter/plan.v1",
            "runx/twitter.plan-result.v1",
            "runx/twitter..plan.v1",
        ] {
            assert!(!is_packet_id(id), "{id:?}");
        }
    }

    #[test]
    fn a_package_id_names_its_owner_file_and_url() -> Result<(), &'static str> {
        let id = PacketId::new("runx/github-issue-to-pr.host_result.v1").ok_or("id")?;
        assert!(!id.is_core());
        assert_eq!(id.owner(), Some(("runx", "github-issue-to-pr")));
        assert_eq!(
            id.file_name().as_deref(),
            Some("host_result.v1.schema.json")
        );
        assert_eq!(
            id.pinned_path().as_deref(),
            Some("runx/github-issue-to-pr/host_result.v1.schema.json")
        );
        assert_eq!(
            id.schema_url(),
            "https://schemas.runx.ai/packages/runx/github-issue-to-pr/host-result/v1.json"
        );

        let core = PacketId::new("runx.payment.cancel_purchase.request.v1").ok_or("core")?;
        assert!(core.is_core());
        assert_eq!(core.owner(), None);
        assert_eq!(core.file_name(), None);
        assert_eq!(
            core.schema_url(),
            "https://schemas.runx.ai/runx/payment/cancel-purchase/request/v1.json"
        );
        Ok(())
    }
}
