use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::{
    RUNX_CWD_ENV, RUNX_HOME_ENV, RUNX_PROJECT_DIR_ENV, RUNX_RECEIPT_DIR_ENV, RunxPaths, isolate,
};

fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect()
}

#[test]
fn every_location_follows_from_three_roots() {
    let paths = RunxPaths::resolve(
        &env(&[("HOME", "/home/kam"), (RUNX_CWD_ENV, "/work/app")]),
        Path::new("/work/app/skills"),
    );

    assert_eq!(paths.cwd(), Path::new("/work/app/skills"));
    assert_eq!(paths.workspace(), Path::new("/work/app"));
    assert_eq!(paths.home(), Path::new("/home/kam/.runx"));
    assert_eq!(paths.project(), Path::new("/work/app/.runx"));
    assert_eq!(
        paths.config_file(),
        Path::new("/home/kam/.runx/config.json")
    );
    assert_eq!(paths.receipts(), Path::new("/work/app/.runx/receipts"));
    assert_eq!(paths.receipts_override(), None);
    assert_eq!(
        paths.authoring_applications(),
        Path::new("/work/app/.runx/authoring/applications")
    );
    assert_eq!(paths.trust_records(), Path::new("/home/kam/.runx/trust"));
    assert_eq!(paths.bundled_skills(), Path::new("/home/kam/.runx/bundled"));
    assert_eq!(paths.tier_statements(), Path::new("/home/kam/.runx/tiers"));
    assert_eq!(
        paths.registry_keys_file(),
        Path::new("/home/kam/.runx/registry-keys.json")
    );
}

#[test]
fn relative_roots_resolve_against_the_workspace() {
    let paths = RunxPaths::resolve(
        &env(&[
            (RUNX_CWD_ENV, "app"),
            (RUNX_HOME_ENV, "state/home"),
            (RUNX_PROJECT_DIR_ENV, "state/project"),
            (RUNX_RECEIPT_DIR_ENV, "../receipts"),
        ]),
        Path::new("/work"),
    );

    assert_eq!(paths.workspace(), Path::new("/work/app"));
    assert_eq!(paths.home(), Path::new("/work/app/state/home"));
    assert_eq!(paths.project(), Path::new("/work/app/state/project"));
    assert_eq!(paths.receipts(), Path::new("/work/receipts"));
    assert_eq!(paths.in_workspace("/abs/file"), Path::new("/abs/file"));
}

#[test]
fn without_a_user_home_every_directory_of_a_workspace_shares_its_home() {
    let paths = RunxPaths::resolve(
        &env(&[(RUNX_CWD_ENV, "/work/app")]),
        Path::new("/work/app/plugin/skills"),
    );

    assert_eq!(paths.cwd(), Path::new("/work/app/plugin/skills"));
    assert_eq!(paths.home(), Path::new("/work/app/.runx"));
}

/// A user home that is empty or relative is no home: the runx home falls back
/// to the workspace as it does with none, and never to a path relative to
/// wherever runx happens to run. An absolute `USERPROFILE` still stands in.
#[test]
fn an_empty_or_relative_user_home_counts_as_none() {
    let fallback = RunxPaths::resolve(&env(&[(RUNX_CWD_ENV, "/work/app")]), Path::new("/work/app"));
    assert_eq!(fallback.home(), Path::new("/work/app/.runx"));
    for home in ["", "home/kam", "./home", "~"] {
        let paths = RunxPaths::resolve(
            &env(&[("HOME", home), (RUNX_CWD_ENV, "/work/app")]),
            Path::new("/work/app"),
        );
        assert_eq!(paths.home(), fallback.home(), "HOME={home:?}");
        let paths = RunxPaths::resolve(
            &env(&[
                ("HOME", home),
                ("USERPROFILE", "/home/kam"),
                (RUNX_CWD_ENV, "/work/app"),
            ]),
            Path::new("/work/app"),
        );
        assert_eq!(paths.home(), Path::new("/home/kam/.runx"), "HOME={home:?}");
    }
}

/// The home is rooted when `RUNX_HOME` is set or a user home is absolute;
/// otherwise it fell back to the workspace, which a repository can ship, and
/// callers are told so (D30).
#[test]
fn a_home_that_fell_back_to_the_workspace_says_so() {
    let fell_back = |pairs: &[(&str, &str)]| {
        RunxPaths::resolve(&env(pairs), Path::new("/work/app")).home_fell_back()
    };
    assert!(fell_back(&[(RUNX_CWD_ENV, "/work/app")]));
    assert!(fell_back(&[
        ("HOME", "home/kam"),
        (RUNX_CWD_ENV, "/work/app")
    ]));
    assert!(fell_back(&[
        (RUNX_HOME_ENV, " "),
        (RUNX_CWD_ENV, "/work/app")
    ]));
    assert!(!fell_back(&[
        ("HOME", "/home/kam"),
        (RUNX_CWD_ENV, "/work/app")
    ]));
    assert!(!fell_back(&[("USERPROFILE", "/home/kam")]));
    // A home named outright is the operator's, wherever it sits.
    assert!(!fell_back(&[(RUNX_HOME_ENV, "state/home")]));
    assert!(!fell_back(&[(RUNX_HOME_ENV, "/work/app/.runx")]));
}

#[test]
fn a_blank_root_is_unset() {
    let paths = RunxPaths::resolve(
        &env(&[
            ("HOME", "/home/kam"),
            (RUNX_CWD_ENV, "/work/app"),
            (RUNX_HOME_ENV, " "),
            (RUNX_PROJECT_DIR_ENV, ""),
        ]),
        Path::new("/work/app"),
    );

    assert_eq!(paths.home(), Path::new("/home/kam/.runx"));
    assert_eq!(paths.project(), Path::new("/work/app/.runx"));
}

#[test]
fn the_workspace_is_the_nearest_marked_directory() -> std::io::Result<()> {
    // A repository marks a project, as a skills/ directory does; nothing is
    // initialized to mark one.
    let root = tempfile::tempdir()?;
    let workspace = root.path().join("app");
    let nested = workspace.join("src").join("deep");
    std::fs::create_dir_all(workspace.join(".git"))?;
    std::fs::create_dir_all(&nested)?;

    let marked = RunxPaths::resolve(&BTreeMap::new(), &nested);
    let unmarked = RunxPaths::resolve(&BTreeMap::new(), root.path());

    assert_eq!(marked.workspace(), workspace);
    assert_eq!(unmarked.workspace(), root.path());
    Ok(())
}

/// The filesystem root is no one's workspace: a machine's `/skills` marks no
/// project above an unmarked directory, and from the root itself the user's
/// home stands in, so nothing is written beside the root.
#[test]
fn the_filesystem_root_is_no_ones_workspace() {
    let root = Path::new("/");
    assert!(
        super::find_workspace(Path::new("/definitely/not/a/project"))
            .is_none_or(|found| found != root)
    );
    let from_root = RunxPaths::resolve(&env(&[("HOME", "/home/u")]), root);
    assert_eq!(from_root.workspace(), Path::new("/home/u"));
    assert_eq!(from_root.project(), Path::new("/home/u/.runx"));
    let unmarked = RunxPaths::resolve(
        &env(&[("HOME", "/home/u")]),
        Path::new("/definitely/not/a/project"),
    );
    assert_eq!(unmarked.workspace(), Path::new("/definitely/not/a/project"));
}

#[test]
fn an_otherwise_unmarked_plugin_is_its_own_workspace() -> std::io::Result<()> {
    // Only where nothing else marks a project does the plugin, the nearest
    // directory holding connectors/, mark one.
    let root = tempfile::tempdir()?;
    let plugin = root.path().join("plugin");
    let skill = plugin.join("work");
    std::fs::create_dir_all(plugin.join("connectors"))?;
    std::fs::create_dir_all(&skill)?;

    assert_eq!(
        RunxPaths::resolve(&BTreeMap::new(), &skill).workspace(),
        plugin
    );

    std::fs::create_dir_all(root.path().join(".git"))?;
    assert_eq!(
        RunxPaths::resolve(&BTreeMap::new(), &skill).workspace(),
        root.path()
    );
    Ok(())
}

#[test]
fn test_receipts_follow_the_request_then_the_environment() {
    let base = env(&[(RUNX_CWD_ENV, "/work/app")]);
    let mut overridden = base.clone();
    overridden.insert(RUNX_RECEIPT_DIR_ENV.to_owned(), "/receipts".to_owned());

    let paths = RunxPaths::resolve(&base, Path::new("/work/app"));
    let with_env = RunxPaths::resolve(&overridden, Path::new("/work/app"));

    assert_eq!(
        paths.test_receipts(None),
        Path::new("/work/app/.runx/test-receipts")
    );
    assert_eq!(with_env.test_receipts(None), Path::new("/receipts"));
    assert_eq!(
        with_env.test_receipts(Some(Path::new("out"))),
        Path::new("/work/app/out")
    );
}

#[test]
fn an_isolated_run_keeps_its_home_and_project_in_its_root() {
    let mut run_env = env(&[
        (RUNX_CWD_ENV, "/work/app"),
        (RUNX_HOME_ENV, "/home/kam/.runx"),
        (RUNX_PROJECT_DIR_ENV, "/work/app/.runx"),
    ]);
    isolate(&mut run_env, Path::new("/tmp/case"));
    let paths = RunxPaths::resolve(&run_env, Path::new("/work/app"));

    assert_eq!(paths.home(), PathBuf::from("/tmp/case/home"));
    assert_eq!(paths.project(), PathBuf::from("/tmp/case/project"));
    assert_eq!(paths.workspace(), Path::new("/work/app"));
}
