//! Filesystem reconciliation for schema artifacts supplied by contract owners.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use crate::SchemaArtifact;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SchemaDrift {
    pub stale: Vec<&'static str>,
    pub orphans: Vec<String>,
}

impl SchemaDrift {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.stale.is_empty() && self.orphans.is_empty()
    }
}

pub fn reconcile_schema_artifacts(
    out_dir: &Path,
    check: bool,
    artifacts: Vec<SchemaArtifact>,
) -> Result<SchemaDrift, std::io::Error> {
    fs::create_dir_all(out_dir)?;

    let expected_file_names = artifacts
        .iter()
        .map(|artifact| artifact.file_name)
        .collect::<BTreeSet<_>>();
    if expected_file_names.len() != artifacts.len() {
        return Err(std::io::Error::other(
            "contract owners declared duplicate schema artifact filenames",
        ));
    }

    let mut stale = Vec::new();
    for artifact in artifacts {
        let path = out_dir.join(artifact.file_name);
        let generated = artifact.document().map_err(std::io::Error::other)?;
        if check {
            match fs::read_to_string(&path) {
                Ok(current) if current == generated => {}
                _ => stale.push(artifact.file_name),
            }
        } else {
            fs::write(path, generated)?;
        }
    }

    let orphans = orphan_schema_files(out_dir, &expected_file_names)?;
    if !check {
        for file_name in orphans {
            fs::remove_file(out_dir.join(file_name))?;
        }
        return Ok(SchemaDrift::default());
    }

    Ok(SchemaDrift { stale, orphans })
}

fn orphan_schema_files(
    out_dir: &Path,
    expected_file_names: &BTreeSet<&'static str>,
) -> Result<Vec<String>, std::io::Error> {
    let mut orphans = Vec::new();
    for entry in fs::read_dir(out_dir)? {
        let entry = entry?;
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();
        if file_name.ends_with(".schema.json") && !expected_file_names.contains(file_name.as_ref())
        {
            orphans.push(file_name.into_owned());
        }
    }
    orphans.sort();
    Ok(orphans)
}
