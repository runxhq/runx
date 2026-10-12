//! Native runtime support for `runx dev` fixture loops.

pub mod r#loop;
mod materialize;
pub mod presentation;
mod skill;
mod support;
mod tool;
pub mod types;
pub mod watch;

pub use r#loop::{discover_fixture_paths, run_dev_once, run_dev_once_with_executor};
pub use presentation::{DevRenderTheme, render_dev_result, render_dev_result_with_theme};
pub use runx_parser::DevFixtureLane;
pub use types::{
    DevError, DevFixtureAssertion, DevFixtureAssertionKind, DevFixtureExecutionRoots,
    DevFixtureExecutor, DevFixtureResult, DevFixtureStatus, DevLoopOptions, DevReport,
    DevReportStatus, LoadedDevFixture, LocalDevFixtureExecutor, PreparedDevFixtureWorkspace,
};
pub use watch::{
    DEFAULT_DEV_WATCH_DEBOUNCE_MS, DevWatchError, DevWatchEvent, DevWatchEventKind,
    DevWatchOptions, DevWatchSnapshot, DevWatchTrigger, PollingDevWatcher, collect_watch_snapshot,
    should_ignore_dev_watch_path,
};
