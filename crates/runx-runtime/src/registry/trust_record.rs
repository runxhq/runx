//! A person's trust (identity contract, "Trust"): a record in their runx home
//! of each execution closure they trusted, by its digest, so a later load of
//! that exact closure, from any workspace sharing the home, does not ask
//! again. The digest leaves runx's version out, so an upgrade keeps it. A
//! repository can't ship one and a signature can't carry one, and a home that
//! fell back to the workspace holds none.

use std::path::PathBuf;

use runx_contracts::TrustTier;
use serde::{Deserialize, Serialize};

use crate::RunxPaths;
use crate::private_file;

/// The most a trust record runx writes holds.
const MAX_TRUST_RECORD_BYTES: u64 = 64 * 1024;

/// An execution closure a person trusted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustRecord {
    pub closure_digest: String,
    /// The package it is the closure of, as the load named it.
    pub package: String,
    /// The package's tier when the person trusted it; none for the project's
    /// own, which has no tier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<TrustTier>,
}

/// Whether a person trusted the execution closure `closure_digest`.
///
/// # Errors
///
/// Refuses a home that fell back, and fails on a record that can't be read.
pub(crate) fn trusted(paths: &RunxPaths, closure_digest: &str) -> Result<bool, String> {
    let path = trust_path(paths, closure_digest)?;
    let unreadable = |problem: String| format!("{}: {problem}", path.display());
    let Some(bytes) = private_file::read_private(&path, MAX_TRUST_RECORD_BYTES)
        .map_err(|fault| unreadable(fault.to_string()))?
    else {
        return Ok(false);
    };
    let record: TrustRecord =
        serde_json::from_slice(&bytes).map_err(|error| unreadable(error.to_string()))?;
    Ok(record.closure_digest == closure_digest)
}

/// Record that a person trusted `record`'s closure.
///
/// # Errors
///
/// Refuses a home that fell back, and fails when the record can't be written.
pub(crate) fn record_trust(paths: &RunxPaths, record: &TrustRecord) -> Result<(), String> {
    let path = trust_path(paths, &record.closure_digest)?;
    let unwritable = |problem: String| format!("{}: {problem}", path.display());
    let directory = path
        .parent()
        .ok_or_else(|| unwritable("has no directory".to_owned()))?;
    private_file::ensure_private_directory(directory)
        .map_err(|fault| unwritable(fault.to_string()))?;
    let encoded = serde_json::to_vec_pretty(record).map_err(|error| error.to_string())?;
    private_file::replace_private(&path, &encoded).map_err(|fault| unwritable(fault.to_string()))
}

/// `<home>/trust/<the closure digest's hex>.json`, refused in a home that
/// fell back to the workspace, which a repository can ship.
fn trust_path(paths: &RunxPaths, closure_digest: &str) -> Result<PathBuf, String> {
    if paths.home_fell_back() {
        return Err(format!(
            "runx's home fell back to the workspace, at {}, and a home a repository can ship holds no one's trust; set HOME to your home directory, or RUNX_HOME to a runx home",
            paths.home().display()
        ));
    }
    Ok(paths.trust_records().join(format!(
        "{}.json",
        closure_digest.trim_start_matches("sha256:")
    )))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    const CLOSURE: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn record() -> TrustRecord {
        TrustRecord {
            closure_digest: CLOSURE.to_owned(),
            package: "connector marker".to_owned(),
            tier: None,
        }
    }

    fn paths(workspace: &std::path::Path, home: Option<&std::path::Path>) -> RunxPaths {
        let mut env = BTreeMap::from([(
            crate::paths::RUNX_CWD_ENV.to_owned(),
            workspace.to_string_lossy().into_owned(),
        )]);
        if let Some(home) = home {
            env.insert(
                crate::paths::RUNX_HOME_ENV.to_owned(),
                home.to_string_lossy().into_owned(),
            );
        }
        RunxPaths::resolve(&env, workspace)
    }

    #[test]
    fn trust_is_recorded_in_the_home_and_read_back() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let home = temp.path().join("home");
        let paths = paths(&temp.path().join("work"), Some(&home));
        assert!(!trusted(&paths, CLOSURE)?);
        record_trust(&paths, &record())?;
        assert!(trusted(&paths, CLOSURE)?);
        let written: serde_json::Value = serde_json::from_slice(&std::fs::read(
            home.join("trust")
                .join(format!("{}.json", CLOSURE.trim_start_matches("sha256:"))),
        )?)?;
        assert_eq!(
            written,
            serde_json::json!({"closure_digest": CLOSURE, "package": "connector marker"})
        );
        assert!(!paths.project().join("trust").exists());
        Ok(())
    }

    /// With no home of its own, the home is `<workspace>/.runx`, where a
    /// repository can ship a record: neither reading nor recording trust
    /// happens there.
    #[test]
    fn a_home_that_fell_back_neither_reads_nor_records_trust()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let paths = paths(temp.path(), None);
        let shipped = paths.home().join("trust");
        std::fs::create_dir_all(&shipped)?;
        let planted = shipped.join(format!("{}.json", CLOSURE.trim_start_matches("sha256:")));
        std::fs::write(&planted, serde_json::to_vec(&record())?)?;

        let read = trusted(&paths, CLOSURE);
        assert!(
            read.as_ref()
                .is_err_and(|reason| reason.contains("set HOME")),
            "{read:?}"
        );
        std::fs::remove_file(&planted)?;
        let recorded = record_trust(&paths, &record());
        assert!(
            recorded
                .as_ref()
                .is_err_and(|reason| reason.contains("set HOME")),
            "{recorded:?}"
        );
        assert!(!planted.exists(), "nothing is recorded");
        Ok(())
    }
}
