use std::fs;

use runx_contracts::JsonValue;

use super::text_bundle_effect;
use crate::filesystem::{TextBundle, TextFileWrite};
use crate::services::WorkspaceBounds;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn an_edit_shows_each_file_it_changes_creates_and_deletes() -> TestResult {
    let workspace = tempfile::tempdir()?;
    let root = workspace.path();
    fs::write(root.join("kept.rs"), "one\ntwo\n")?;
    fs::write(root.join("gone.rs"), "gone\n")?;
    let effect = text_bundle_effect(
        "fs.apply_bundle",
        root,
        &TextBundle {
            writes: vec![
                TextFileWrite {
                    path: "kept.rs".to_owned(),
                    contents: "one\nthree\n".to_owned(),
                },
                TextFileWrite {
                    path: "new/made.rs".to_owned(),
                    contents: "made\n".to_owned(),
                },
            ],
            deletes: vec!["gone.rs".to_owned()],
        },
        WorkspaceBounds::Agent,
    )?
    .ok_or("the bundle changes files")?;
    assert_eq!(effect.paths, ["kept.rs", "new/made.rs", "gone.rs"]);
    assert!(effect.argv.is_none());
    let files = effect
        .payload
        .as_object()
        .and_then(|payload| payload.get("files"))
        .and_then(JsonValue::as_array)
        .ok_or("the files changed")?;
    let file = |index: usize, field: &str| {
        files[index]
            .as_object()
            .and_then(|file| file.get(field))
            .and_then(JsonValue::as_str)
            .map(str::to_owned)
    };
    assert!(file(0, "diff").is_some_and(|diff| diff.contains("-two") && diff.contains("+three")));
    assert!(file(0, "before_digest").is_some() && file(0, "after_digest").is_some());
    assert!(
        file(1, "before_digest").is_none(),
        "a created file replaces nothing"
    );
    assert!(
        file(2, "after_digest").is_none(),
        "a deleted file becomes nothing"
    );
    assert_eq!(
        fs::read_to_string(root.join("kept.rs"))?,
        "one\ntwo\n",
        "describing an edit changes nothing"
    );

    let secret = text_bundle_effect(
        "fs.write",
        root,
        &TextBundle {
            writes: vec![TextFileWrite {
                path: ".env".to_owned(),
                contents: "TOKEN=1".to_owned(),
            }],
            deletes: Vec::new(),
        },
        WorkspaceBounds::Agent,
    );
    assert!(
        secret.is_err(),
        "an edit is admitted as applying it would be"
    );
    assert!(
        text_bundle_effect(
            "fs.apply_bundle",
            root,
            &TextBundle::default(),
            WorkspaceBounds::Agent
        )?
        .is_none(),
        "an empty bundle changes nothing"
    );
    Ok(())
}
