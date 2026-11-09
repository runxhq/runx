//! What a run is started by: the digest of the sources each binary was built
//! from (reuse contract, `implementation`). The runtime's is made by its
//! build script, and so is the worker's it was built beside, which the
//! worker reports when it starts, so a worker of other sources is refused. A
//! run records its implementation when it starts, and a resume under another
//! is refused, so a build of changed sources never replays old history.

use runx_contracts::RunImplementation;

/// The implementation this runx is.
#[must_use]
pub fn current() -> RunImplementation {
    RunImplementation {
        runtime: env!("RUNX_RUNTIME_SOURCE_DIGEST").into(),
        worker: expected_worker().map(Into::into),
    }
}

/// The digest of the worker's sources this runtime was built beside; none
/// for a build without them to hand.
pub(crate) fn expected_worker() -> Option<&'static str> {
    option_env!("RUNX_JS_WORKER_SOURCE_DIGEST")
}

/// Why a run started under `started` can't go on under `current`: a run
/// resumes only under the implementation that started it.
pub(crate) fn refusal(
    started: Option<&RunImplementation>,
    current: &RunImplementation,
) -> Option<String> {
    let describe = |implementation: &RunImplementation| match &implementation.worker {
        Some(worker) => format!("runtime {}, worker {worker}", implementation.runtime),
        None => format!("runtime {}, no worker", implementation.runtime),
    };
    match started {
        Some(started) if started == current => None,
        Some(started) => Some(format!(
            "the run was started by {} and this runx is {}; a run resumes only under the implementation that started it",
            describe(started),
            describe(current)
        )),
        None => Some(format!(
            "the run records no implementation, and this runx is {}; a run resumes only under the implementation that started it",
            describe(current)
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use runx_contracts::RunImplementation;

    use super::{current, refusal};

    mod digest {
        include!("../source_digest.rs");
    }

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn write(path: &Path, text: &str) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, text)
    }

    /// The digest reads the crate and the workspace crates it depends on, all
    /// but what no binary is built from, so the same tree gives the same
    /// digest twice, a change to a dependency's source another, and a change
    /// to a test, or to a crate it doesn't depend on, none.
    #[test]
    fn the_same_sources_give_the_same_digest_and_a_changed_one_another() -> TestResult {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        write(&root.join("Cargo.toml"), "[workspace]\n")?;
        write(
            &root.join("runx-app/Cargo.toml"),
            "[package]\nname = \"runx-app\"\n\n[dependencies]\nrunx-lib.workspace = true\nserde = \"1\"\n\n[dev-dependencies]\nrunx-other.workspace = true\n",
        )?;
        write(&root.join("runx-app/src/main.rs"), "fn main() {}\n")?;
        write(&root.join("runx-app/assets/voice.md"), "speak\n")?;
        write(&root.join("runx-app/tests/it.rs"), "#[test] fn it() {}\n")?;
        write(
            &root.join("runx-lib/Cargo.toml"),
            "[package]\nname = \"runx-lib\"\n",
        )?;
        write(&root.join("runx-lib/src/lib.rs"), "pub fn one() {}\n")?;
        write(
            &root.join("runx-other/Cargo.toml"),
            "[package]\nname = \"runx-other\"\n",
        )?;
        write(&root.join("runx-other/src/lib.rs"), "pub fn two() {}\n")?;
        let app = root.join("runx-app");
        let (first, _) = digest::source_digest(&app)?;
        assert_eq!(
            digest::source_digest(&app)?.0,
            first,
            "the same tree, the same digest"
        );
        write(
            &root.join("runx-app/tests/it.rs"),
            "#[test] fn changed() {}\n",
        )?;
        write(&root.join("runx-other/src/lib.rs"), "pub fn three() {}\n")?;
        assert_eq!(
            digest::source_digest(&app)?.0,
            first,
            "a test, and a crate it doesn't build from, change nothing"
        );
        write(&root.join("runx-app/assets/voice.md"), "whisper\n")?;
        let embedded = digest::source_digest(&app)?.0;
        assert_ne!(embedded, first, "a file the crate embeds is its source");
        write(&root.join("runx-lib/src/lib.rs"), "pub fn changed() {}\n")?;
        assert_ne!(
            digest::source_digest(&app)?.0,
            embedded,
            "a dependency's source is the binary's"
        );
        Ok(())
    }

    #[test]
    fn a_resume_under_another_implementation_names_both() {
        let now = current();
        assert_eq!(refusal(Some(&now), &now), None);
        let earlier = RunImplementation {
            runtime: "sha256:earlier".into(),
            worker: now.worker.clone(),
        };
        let refused = refusal(Some(&earlier), &now).unwrap_or_default();
        assert!(
            refused.contains("runtime sha256:earlier") && refused.contains(now.runtime.as_str()),
            "{refused}"
        );
        assert!(refusal(None, &now).is_some(), "a run that records none");
    }
}
