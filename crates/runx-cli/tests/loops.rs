//! `runx loop new`: a loop from the template its skill ships, checked as a
//! loop before it's written, and never over a loop that's there.

use std::fs;

use crate::support::{repo_root, temp_root, unsigned_runx_command_at};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn runx(root: &std::path::Path, args: &[&str]) -> TestResult<std::process::Output> {
    let mut command = unsigned_runx_command_at(root);
    command.env("RUNX_CWD", root).args(args);
    crate::support::isolate_project(&mut command);
    Ok(command.output()?)
}

#[test]
fn a_loop_starts_from_the_template_its_skill_ships() -> TestResult {
    let root = temp_root("runx-loop-new");
    fs::create_dir_all(&root)?;

    let made = runx(&root, &["loop", "new", "runx/reflect", "daily"])?;
    assert!(
        made.status.success(),
        "{}",
        String::from_utf8_lossy(&made.stderr)
    );
    let written = fs::read(root.join("loops/daily.yaml"))?;
    assert_eq!(
        written,
        fs::read(repo_root()?.join("skills/reflect/loops/reflect.yaml"))?
    );
    let file = runx_loops::LoopFile::parse(&written)?;
    assert_eq!(file.calls.to_string(), "runx/reflect#reflect");

    let again = runx(&root, &["loop", "new", "runx/reflect", "daily"])?;
    assert!(!again.status.success());
    assert!(String::from_utf8_lossy(&again.stderr).contains("exists"));

    let none = runx(&root, &["loop", "new", "runx/memory", "remembering"])?;
    assert!(!none.status.success());
    assert_eq!(
        String::from_utf8_lossy(&none.stderr),
        "runx: runx/memory ships no loop template\n"
    );
    assert!(!root.join("loops/remembering.yaml").exists());
    Ok(())
}

#[test]
fn a_template_that_isn_t_a_loop_is_never_copied() -> TestResult {
    let root = temp_root("runx-loop-template");
    let package = root.join("skills/watch");
    fs::create_dir_all(package.join("loops"))?;
    fs::write(
        package.join("SKILL.md"),
        "---\nname: watch\ndescription: Watch a page.\n---\n# watch\n\nWatch the page.\n",
    )?;
    fs::write(
        package.join("loops/daily.yaml"),
        "calls: watch\nbrief: Watch the page.\n",
    )?;

    let made = runx(&root, &["loop", "new", "./skills/watch", "daily"])?;
    assert!(!made.status.success());
    let said = String::from_utf8_lossy(&made.stderr);
    assert!(
        said.starts_with("runx: ./skills/watch's loop template isn't a loop: "),
        "{said}"
    );
    assert!(!root.join("loops/daily.yaml").exists());
    Ok(())
}
