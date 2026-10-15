use std::collections::BTreeMap;
use std::fs;

use super::{BundleEvent, Journal, Outcome, recover};
use crate::filesystem::admission::{admit_text_bundle, missing_parent_directories, snapshot_paths};
use crate::filesystem::apply::apply_admitted_bundle;
use crate::filesystem::{Expected, TextBundle, TextFileWrite, apply_text_bundle};
use crate::paths::{RUNX_CWD_ENV, RunxPaths};
use crate::services::WorkspaceBounds;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn bundle() -> TextBundle {
    TextBundle {
        writes: vec![
            TextFileWrite {
                path: "kept.md".to_owned(),
                contents: "after\n".to_owned(),
            },
            TextFileWrite {
                path: "made/new.md".to_owned(),
                contents: "new\n".to_owned(),
            },
        ],
        deletes: vec!["gone.md".to_owned()],
        expected: BTreeMap::from([
            (
                "kept.md".to_owned(),
                Expected::Digest(runx_contracts::sha256_prefixed(b"before\n")),
            ),
            (
                "gone.md".to_owned(),
                Expected::Digest(runx_contracts::sha256_prefixed(b"gone\n")),
            ),
        ]),
    }
}

#[test]
fn a_bundle_a_crash_left_begun_is_rolled_back_by_the_next_driver() -> TestResult {
    let workspace = tempfile::tempdir()?;
    let root = workspace.path();
    fs::write(root.join("kept.md"), "before\n")?;
    fs::write(root.join("gone.md"), "gone\n")?;
    let env = BTreeMap::from([
        (RUNX_CWD_ENV.to_owned(), root.to_string_lossy().into_owned()),
        (crate::RUNX_RUN_ID_ENV.to_owned(), "run_journal".to_owned()),
    ]);
    let journal = Journal::of_run(&env, root)?.ok_or("the run has a journal")?;

    // A driver journaled the bundle, applied it, and died before it ended.
    let admitted = admit_text_bundle("fs.apply_bundle", root, &bundle(), WorkspaceBounds::Agent)?;
    let snapshots = snapshot_paths(&admitted)?;
    let created = missing_parent_directories(&admitted)?;
    journal.begin(&admitted.operation, &admitted.root, &snapshots, &created)?;
    apply_admitted_bundle(&admitted, &snapshots)?;
    assert!(!root.join("gone.md").exists());

    let store = crate::kernel::store::KernelStore::for_project(&RunxPaths::resolve(&env, root))?;
    recover(&store, "run_journal")?;
    assert_eq!(fs::read_to_string(root.join("kept.md"))?, "before\n");
    assert_eq!(fs::read_to_string(root.join("gone.md"))?, "gone\n");
    assert!(!root.join("made").exists(), "a directory it made is gone");
    let events = store.read::<BundleEvent>("run_journal")?;
    assert!(matches!(
        events.last().map(|recorded| &recorded.event),
        Some(BundleEvent::Ended(ended)) if ended.outcome == Outcome::RolledBack
    ));
    // Recovering again changes nothing.
    recover(&store, "run_journal")?;
    assert_eq!(
        store.read::<BundleEvent>("run_journal")?.len(),
        events.len()
    );

    // A bundle applied whole records that it ended, and names its entry.
    let report = apply_text_bundle(
        "fs.apply_bundle",
        root,
        &bundle(),
        WorkspaceBounds::Agent,
        Some(&journal),
    )?;
    assert!(report.get("journal").is_some());
    recover(&store, "run_journal")?;
    assert_eq!(fs::read_to_string(root.join("kept.md"))?, "after\n");
    Ok(())
}
