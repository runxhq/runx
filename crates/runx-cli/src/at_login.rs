//! Staying on across login (the loop contract §15): `runx on` asks once, at
//! a terminal, whether runx comes on when the person logs in, and keeps the
//! answer in the home so it's never asked again. Yes writes one unit for
//! this runx home that runs `runx on --foreground` at login with this binary
//! and this home: a launchd agent on macOS, an enabled systemd user unit on
//! Linux. It starts runx at login and never restarts it, so `runx off` stays
//! off until the next login. No writes it away.

use std::io::{BufRead as _, IsTerminal as _, Write as _};
use std::path::{Path, PathBuf};

/// Where the answer is kept in the always-on runx's folder.
const ANSWER: &str = "at-login";

/// The operating systems a login unit is written for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Os {
    Macos,
    Linux,
}

impl Os {
    pub(crate) fn current() -> Option<Self> {
        if cfg!(target_os = "macos") {
            Some(Self::Macos)
        } else if cfg!(target_os = "linux") {
            Some(Self::Linux)
        } else {
            None
        }
    }
}

/// One runx home's login unit: the file, what it holds, and the link that
/// enables it, for systemd.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Unit {
    pub path: PathBuf,
    pub contents: String,
    pub enabled_by: Option<PathBuf>,
}

/// Where the person's own settings are: their home, and their config folder.
pub(crate) struct UserDirs {
    pub home: PathBuf,
    pub config: PathBuf,
}

impl UserDirs {
    pub(crate) fn from_env(env: &std::collections::BTreeMap<String, String>) -> Option<Self> {
        let home = PathBuf::from(env.get("HOME")?);
        let config = env
            .get("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"));
        Some(Self { home, config })
    }
}

/// This runx home's name among units: one per home, so two homes never
/// share one.
fn label(runx_home: &Path) -> String {
    let digest = ring::digest::digest(
        &ring::digest::SHA256,
        runx_home.to_string_lossy().as_bytes(),
    );
    let short = digest.as_ref()[..4]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("dev.runx.on.{short}")
}

fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn quoted(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The unit that brings runx on at login for `runx_home`, with `binary`.
pub(crate) fn unit(os: Os, user: &UserDirs, runx_home: &Path, binary: &Path) -> Unit {
    let name = label(runx_home);
    let log = runx_home.join("always-on").join("log");
    match os {
        Os::Macos => Unit {
            path: user
                .home
                .join("Library/LaunchAgents")
                .join(format!("{name}.plist")),
            contents: format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{name}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{binary}</string>
    <string>on</string>
    <string>--foreground</string>
  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>RUNX_HOME</key>
    <string>{home}</string>
  </dict>
  <key>WorkingDirectory</key>
  <string>{user}</string>
  <key>RunAtLoad</key>
  <true/>
  <key>StandardOutPath</key>
  <string>{log}</string>
  <key>StandardErrorPath</key>
  <string>{log}</string>
</dict>
</plist>
"#,
                binary = xml(&binary.to_string_lossy()),
                home = xml(&runx_home.to_string_lossy()),
                user = xml(&user.home.to_string_lossy()),
                log = xml(&log.to_string_lossy()),
            ),
            enabled_by: None,
        },
        Os::Linux => {
            let file = format!("{name}.service");
            let units = user.config.join("systemd/user");
            Unit {
                path: units.join(&file),
                contents: format!(
                    "[Unit]\nDescription=runx, on for the runx home {home}\n\n[Service]\nType=simple\nExecStart={binary} on --foreground\nEnvironment={environment}\nWorkingDirectory=%h\nRestart=no\n\n[Install]\nWantedBy=default.target\n",
                    home = runx_home.display(),
                    binary = quoted(&binary.to_string_lossy()),
                    environment = quoted(&format!("RUNX_HOME={}", runx_home.display())),
                ),
                enabled_by: Some(units.join("default.target.wants").join(file)),
            }
        }
    }
}

/// Write `unit`, and enable it.
pub(crate) fn install(unit: &Unit) -> Result<(), String> {
    if let Some(parent) = unit.path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("making {}: {error}", parent.display()))?;
    }
    std::fs::write(&unit.path, &unit.contents)
        .map_err(|error| format!("writing {}: {error}", unit.path.display()))?;
    if let Some(link) = &unit.enabled_by {
        if let Some(parent) = link.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("making {}: {error}", parent.display()))?;
        }
        if std::fs::symlink_metadata(link).is_err() {
            #[cfg(unix)]
            std::os::unix::fs::symlink(&unit.path, link)
                .map_err(|error| format!("enabling {}: {error}", link.display()))?;
        }
    }
    Ok(())
}

/// Take `unit` away, enabled or not.
pub(crate) fn remove(unit: &Unit) -> Result<(), String> {
    for path in unit.enabled_by.iter().chain([&unit.path]) {
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("removing {}: {error}", path.display())),
        }
    }
    Ok(())
}

/// The answer kept in `dir`, when the person gave one.
pub(crate) fn kept(dir: &Path) -> Option<bool> {
    match std::fs::read_to_string(dir.join(ANSWER)).ok()?.trim() {
        "yes" => Some(true),
        "no" => Some(false),
        _ => None,
    }
}

fn keep(dir: &Path, yes: bool) -> Result<(), String> {
    std::fs::write(dir.join(ANSWER), if yes { "yes\n" } else { "no\n" })
        .map_err(|error| format!("keeping the answer: {error}"))
}

/// Ask once, at a terminal.
fn ask() -> Option<bool> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return None;
    }
    let _ = crate::cli_io::write_stdout("Keep runx on when you log in? [y/N] ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line).ok()?;
    Some(matches!(line.trim().to_lowercase().as_str(), "y" | "yes"))
}

/// Settle whether runx comes on at login: `given` outright, or the kept
/// answer, or asked once at a terminal. Returns the line `runx on` says.
pub(crate) fn settle(
    dir: &Path,
    given: Option<bool>,
    unit: Option<Unit>,
) -> Result<Option<String>, String> {
    let answer = match given.or_else(|| kept(dir)) {
        Some(answer) => answer,
        None => match ask() {
            Some(answer) => answer,
            None => {
                return Ok(Some(
                    "runx stays on until you log out or switch it off: `runx on --at-login` keeps it on across login".to_owned(),
                ));
            }
        },
    };
    // A kept answer that's settled says nothing again.
    let asked_now = given.is_some() || kept(dir).is_none();
    let Some(unit) = unit else {
        keep(dir, answer)?;
        return Ok(asked_now.then(|| "runx can't come on at login on this system".to_owned()));
    };
    if answer {
        install(&unit)?;
    } else {
        remove(&unit)?;
    }
    keep(dir, answer)?;
    Ok(asked_now.then(|| {
        if answer {
            format!("runx comes on when you log in ({})", unit.path.display())
        } else {
            "runx doesn't come on when you log in".to_owned()
        }
    }))
}

/// A line saying whether runx comes on at login, for `runx status`.
pub(crate) fn status_line(dir: &Path, unit: Option<&Unit>, binary: &Path) -> String {
    match (kept(dir), unit) {
        (Some(true), Some(unit)) if unit.path.exists() => {
            if binary.exists() {
                "runx comes on when you log in".to_owned()
            } else {
                format!(
                    "the login unit at {} names {}, which is gone: `runx on --at-login` writes it again",
                    unit.path.display(),
                    binary.display()
                )
            }
        }
        (Some(true), Some(unit)) => format!(
            "runx was to come on at login, and {} is gone: `runx on --at-login` writes it again",
            unit.path.display()
        ),
        _ => "runx doesn't come on at login: `runx on --at-login`".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirs(root: &Path) -> UserDirs {
        UserDirs {
            home: root.join("home"),
            config: root.join("home/.config"),
        }
    }

    #[test]
    fn a_launchd_agent_starts_this_runx_for_this_home_at_login_and_never_restarts_it()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let user = dirs(root.path());
        let runx_home = root.path().join("runx home");
        let made = unit(
            Os::Macos,
            &user,
            &runx_home,
            Path::new("/opt/runx/bin/runx"),
        );
        assert!(
            made.path
                .starts_with(user.home.join("Library/LaunchAgents"))
        );
        assert!(made.path.to_string_lossy().ends_with(".plist"));
        assert!(made.contents.contains("<string>/opt/runx/bin/runx</string>\n    <string>on</string>\n    <string>--foreground</string>"));
        assert!(made.contents.contains(&format!(
            "<key>RUNX_HOME</key>\n    <string>{}</string>",
            runx_home.display()
        )));
        assert!(made.contents.contains("<key>RunAtLoad</key>\n  <true/>"));
        assert!(!made.contents.contains("KeepAlive"), "runx off stays off");
        // Another home has a unit of its own.
        let other = unit(
            Os::Macos,
            &user,
            &root.path().join("another"),
            Path::new("/opt/runx/bin/runx"),
        );
        assert_ne!(made.path, other.path);
        Ok(())
    }

    #[test]
    fn a_systemd_unit_is_enabled_by_its_link_and_taken_away_with_it()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let user = dirs(root.path());
        let made = unit(
            Os::Linux,
            &user,
            &root.path().join("runx"),
            Path::new("/usr/bin/runx"),
        );
        assert!(
            made.contents
                .contains("ExecStart=\"/usr/bin/runx\" on --foreground")
        );
        assert!(made.contents.contains("Restart=no"));
        assert!(made.contents.contains("WantedBy=default.target"));
        install(&made)?;
        let link = made.enabled_by.as_ref().ok_or("enabled by a link")?;
        assert_eq!(std::fs::read_link(link)?, made.path);
        remove(&made)?;
        assert!(!made.path.exists() && std::fs::symlink_metadata(link).is_err());
        Ok(())
    }

    #[test]
    fn the_answer_is_kept_and_never_asked_again() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let dir = root.path().join("always-on");
        std::fs::create_dir_all(&dir)?;
        let user = dirs(root.path());
        let made = || Some(unit(Os::Macos, &user, root.path(), Path::new("/opt/runx")));
        // No terminal and no answer: nothing is written, and it says how.
        let said = settle(&dir, None, made())?.ok_or("it says how")?;
        assert!(said.contains("runx on --at-login"), "{said}");
        assert!(kept(&dir).is_none());
        // Yes writes the unit and keeps the answer; asked again, it says nothing.
        let said = settle(&dir, Some(true), made())?.ok_or("it says so")?;
        assert!(said.starts_with("runx comes on when you log in"), "{said}");
        let path = made().ok_or("a unit")?.path;
        assert!(path.exists());
        assert_eq!(settle(&dir, None, made())?, None);
        assert_eq!(kept(&dir), Some(true));
        // No takes it away, and that's kept too.
        settle(&dir, Some(false), made())?;
        assert!(!path.exists());
        assert_eq!(kept(&dir), Some(false));
        assert_eq!(settle(&dir, None, made())?, None);
        Ok(())
    }

    #[test]
    fn status_says_when_the_unit_names_a_binary_thats_gone()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let dir = root.path().join("always-on");
        std::fs::create_dir_all(&dir)?;
        let user = dirs(root.path());
        let gone = root.path().join("gone/runx");
        let made = unit(Os::Macos, &user, root.path(), &gone);
        assert!(status_line(&dir, Some(&made), &gone).starts_with("runx doesn't come on at login"));
        settle(
            &dir,
            Some(true),
            Some(unit(Os::Macos, &user, root.path(), &gone)),
        )?;
        assert!(status_line(&dir, Some(&made), &gone).contains("which is gone"));
        Ok(())
    }
}
