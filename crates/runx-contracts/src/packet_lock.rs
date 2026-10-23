//! Packet lock contract (`runx.packet.lock.v1`): the digest each published
//! package packet id was frozen at. Bundles carry pinned copies of the package
//! packets they read, so a published id never changes bytes; a changed packet
//! is a new version beside the old one, and the lock only grows.
use serde::{Deserialize, Serialize};

use crate::PacketId;
use crate::connector::Digest;
use crate::schema::RunxSchema;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub enum PacketLockSchema {
    #[serde(rename = "runx.packet.lock.v1")]
    V1,
}

/// One packet id and the digest of its schema's exact bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct PacketLockEntry {
    pub id: PacketId,
    pub digest: Digest,
}

/// Every packet id published from one source tree, ordered by id.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.packet.lock.v1")]
pub struct PacketLock {
    pub schema: PacketLockSchema,
    pub packets: Vec<PacketLockEntry>,
}

#[cfg(test)]
mod tests {
    use super::PacketLock;

    #[test]
    fn the_workspace_lock_is_a_packet_lock_ordered_by_id() -> Result<(), Box<dyn std::error::Error>>
    {
        let lock: PacketLock = serde_json::from_str(&std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packets.lock.json"),
        )?)?;
        assert!(!lock.packets.is_empty());
        assert!(
            lock.packets
                .windows(2)
                .all(|pair| pair[0].id.as_str() < pair[1].id.as_str())
        );
        Ok(())
    }
}
