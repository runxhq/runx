//! A package's `packets/`: the packets it owns, each at
//! `packets/<name>.v<N>.schema.json` under an id naming the package, and the
//! pinned copy of each other package's packet it reads, at
//! `packets/<publisher>/<package>/<name>.v<N>.schema.json`.

use std::collections::BTreeMap;

use runx_contracts::PacketId;

use super::contract::text_file;
use super::{SkillPackageError, SkillPackageSource};
use crate::package::PackagePacket;
use crate::parse_packet_schema_document;

const PACKETS_DIRECTORY: &str = "packets/";

pub(super) fn validate_package_packets(
    source: &SkillPackageSource,
    package: &str,
) -> Result<BTreeMap<PacketId, PackagePacket>, SkillPackageError> {
    let mut packets = BTreeMap::new();
    let mut publisher = None::<String>;
    for (path, contents) in source
        .files
        .range(PACKETS_DIRECTORY.to_owned()..)
        .take_while(|(path, _)| path.starts_with(PACKETS_DIRECTORY))
    {
        let schema = parse_packet_schema_document(path.clone(), text_file(path, contents)?)
            .map_err(|error| SkillPackageError::invalid(path, error.to_string()))?
            .ok_or_else(|| {
                SkillPackageError::invalid(
                    path,
                    "packets/ holds only packet schemas, and this file declares no x-runx-packet-id",
                )
            })?;
        let id = &schema.packet_id;
        let relative = &path[PACKETS_DIRECTORY.len()..];
        let Some((owner_publisher, owner)) = id.owner() else {
            return Err(SkillPackageError::invalid(
                path,
                "a core packet is never a file",
            ));
        };
        let expected = if owner == package {
            if let Some(known) = publisher.as_deref()
                && known != owner_publisher
            {
                return Err(SkillPackageError::invalid(
                    path,
                    format!(
                        "{id} names publisher {owner_publisher}, but this package's packets name {known}"
                    ),
                ));
            }
            publisher = Some(owner_publisher.to_owned());
            id.file_name()
        } else {
            id.pinned_path()
        };
        if expected.as_deref() != Some(relative) {
            return Err(SkillPackageError::invalid(
                path,
                format!(
                    "{id} belongs at packets/{}: a package's own packet is named for its id, and another package's packet is carried only as the pinned copy under the owner's name",
                    expected.unwrap_or_default()
                ),
            ));
        }
        packets.insert(
            id.clone(),
            PackagePacket {
                path: path.clone(),
                schema,
            },
        );
    }
    Ok(packets)
}
