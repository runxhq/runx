use runx_contracts::{JsonValue, StandingApproval, StandingCommands, StandingEdits};

use super::covering;
use crate::capability::LocalEffect;

fn edit(paths: &[&str]) -> LocalEffect {
    LocalEffect {
        payload: JsonValue::Null,
        paths: paths.iter().map(|path| (*path).to_owned()).collect(),
        argv: None,
    }
}

fn command(argv: &[&str]) -> LocalEffect {
    LocalEffect {
        payload: JsonValue::Null,
        paths: Vec::new(),
        argv: Some(argv.iter().map(|token| (*token).to_owned()).collect()),
    }
}

fn edits(paths: &[&str]) -> StandingApproval {
    StandingApproval {
        edits: Some(StandingEdits {
            paths: paths.iter().map(|path| (*path).to_owned()).collect(),
        }),
        ..StandingApproval::default()
    }
}

fn commands(prefix: &[&str]) -> StandingApproval {
    StandingApproval {
        commands: Some(StandingCommands {
            argv_prefix: prefix.iter().map(|token| (*token).into()).collect(),
        }),
        ..StandingApproval::default()
    }
}

#[test]
fn edits_cover_files_under_their_paths_by_whole_components() {
    let src = [edits(&["src"])];
    assert!(covering(&edit(&["src/lib.rs", "src/a/b.rs"]), &src).is_some());
    assert!(covering(&edit(&["srcx/lib.rs"]), &src).is_none());
    assert!(covering(&edit(&["src/lib.rs", "docs/a.md"]), &src).is_none());
    assert!(covering(&edit(&["anything/at/all"]), &[edits(&[""])]).is_some());
    assert!(
        covering(&command(&["cargo", "test"]), &src).is_none(),
        "edits never cover a process"
    );
}

#[test]
fn commands_cover_an_argv_that_begins_with_their_tokens() {
    let tests = [commands(&["cargo", "test"])];
    assert!(covering(&command(&["cargo", "test"]), &tests).is_some());
    assert!(covering(&command(&["cargo", "test", "-p", "runx"]), &tests).is_some());
    assert!(covering(&command(&["cargo", "testing"]), &tests).is_none());
    assert!(covering(&command(&["cargo"]), &tests).is_none());
    assert!(
        covering(&edit(&["Cargo.toml"]), &tests).is_none(),
        "commands never cover an edit"
    );
}

#[test]
fn local_covers_every_local_effect_and_nothing_covers_without_an_entry() {
    let local = [StandingApproval {
        local: true,
        ..StandingApproval::default()
    }];
    assert!(covering(&edit(&["a.rs"]), &local).is_some());
    assert!(covering(&command(&["rm", "-rf", "build"]), &local).is_some());
    assert!(covering(&edit(&["a.rs"]), &[]).is_none());
}
