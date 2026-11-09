//! Filesystem reconciliation for the published schema set.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use crate::{PublishedSchemas, document_text};

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

/// Write every published document to `out_dir` and remove any other schema
/// file there, or under `check` report what differs.
pub fn reconcile_schema_artifacts(
    out_dir: &Path,
    check: bool,
    published: &PublishedSchemas,
) -> Result<SchemaDrift, std::io::Error> {
    fs::create_dir_all(out_dir)?;

    let mut expected_file_names = BTreeSet::new();
    let mut stale = Vec::new();
    for (file_name, document) in published.documents() {
        expected_file_names.insert(file_name);
        let path = out_dir.join(file_name);
        let generated = document_text(document).map_err(std::io::Error::other)?;
        if check {
            match fs::read_to_string(&path) {
                Ok(current) if current == generated => {}
                _ => stale.push(file_name),
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
