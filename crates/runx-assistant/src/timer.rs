//! Owned launchd timer for a finite assistant tick. No resident Runx process.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use runx_runtime::WorkspaceEnv;
use serde_json::{Value, json};

use super::LoadedProfile;

fn home(workspace: &WorkspaceEnv) -> Result<PathBuf, String> {
    workspace
        .env()
        .get("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute() && path.is_dir())
        .ok_or("assistant timer requires an absolute HOME directory".to_owned())
}

fn label(loaded: &LoadedProfile) -> String {
    format!("dev.runx.assistant.{}", loaded.profile.instance_id)
}

fn worker_label(loaded: &LoadedProfile) -> String {
    format!("{}.execute", label(loaded))
}

fn worker_plist(tick_plist: &Path) -> PathBuf {
    tick_plist.with_file_name(format!(
        "{}.execute.plist",
        tick_plist
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("dev.runx.assistant")
    ))
}

fn paths(loaded: &LoadedProfile, workspace: &WorkspaceEnv) -> Result<(PathBuf, PathBuf), String> {
    let home = home(workspace)?;
    Ok((
        home.join("Library")
            .join("LaunchAgents")
            .join(format!("{}.plist", label(loaded))),
        home.join("Library")
            .join("Application Support")
            .join("runx")
            .join("assistant")
            .join(&loaded.profile.instance_id),
    ))
}

pub(super) fn timer_installed(
    loaded: &LoadedProfile,
    workspace: &WorkspaceEnv,
) -> Result<bool, String> {
    let (plist, _) = paths(loaded, workspace)?;
    for path in [&plist, &worker_plist(&plist)] {
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {}
            Ok(_) => return Ok(false),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(format!("reading assistant timer status: {error}")),
        }
    }
    Ok(true)
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn uid() -> Result<String, String> {
    let output = Command::new("/usr/bin/id")
        .arg("-u")
        .output()
        .map_err(|error| format!("reading local uid: {error}"))?;
    if !output.status.success() {
        return Err("reading local uid failed".to_owned());
    }
    let uid = String::from_utf8(output.stdout)
        .map_err(|_| "local uid was not UTF-8")?
        .trim()
        .to_owned();
    if uid.is_empty() || !uid.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("local uid is invalid".to_owned());
    }
    Ok(uid)
}

fn launchctl(args: &[&str]) -> Result<(), String> {
    let output = Command::new("/bin/launchctl")
        .args(args)
        .output()
        .map_err(|error| format!("running launchctl: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "launchctl {} failed: {}",
            args.first().copied().unwrap_or("command"),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

fn copy_binary(source: &Path, destination: &Path) -> Result<(), String> {
    if source == destination {
        return Ok(());
    }
    let temporary = destination.with_extension("tmp");
    if temporary.exists() {
        fs::remove_file(&temporary)
            .map_err(|error| format!("clearing stale assistant binary staging file: {error}"))?;
    }
    fs::copy(source, &temporary)
        .map_err(|error| format!("copying assistant binary {}: {error}", source.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("protecting assistant binary: {error}"))?;
    }
    fs::rename(&temporary, destination)
        .map_err(|error| format!("installing assistant binary: {error}"))?;
    Ok(())
}

fn write_plist(path: &Path, contents: &str) -> Result<(), String> {
    let temporary = path.with_extension("plist.tmp");
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)
        .map_err(|error| format!("opening assistant timer staging file: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("protecting assistant timer: {error}"))?;
    }
    file.write_all(contents.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("writing assistant timer: {error}"))?;
    fs::rename(&temporary, path).map_err(|error| format!("installing assistant timer: {error}"))
}

pub(super) fn install_timer(
    loaded: &LoadedProfile,
    workspace: &WorkspaceEnv,
) -> Result<Value, String> {
    if !cfg!(target_os = "macos") {
        return Err("assistant launchd timer is only available on macOS".to_owned());
    }
    let (plist, instance_dir) = paths(loaded, workspace)?;
    let source_binary = std::env::current_exe()
        .map_err(|error| format!("resolving runx binary: {error}"))?
        .canonicalize()
        .map_err(|error| format!("resolving runx binary: {error}"))?;
    let source_worker = source_binary.with_file_name("runx-js-worker");
    if !source_worker.is_file() {
        return Err(
            "assistant timer requires runx-js-worker beside the current runx binary".to_owned(),
        );
    }
    let bin_dir = instance_dir.join("bin");
    fs::create_dir_all(&bin_dir)
        .map_err(|error| format!("creating assistant installation directory: {error}"))?;
    fs::create_dir_all(plist.parent().ok_or("timer has no parent directory")?)
        .map_err(|error| format!("creating LaunchAgents directory: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&instance_dir, fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("protecting assistant installation: {error}"))?;
        fs::set_permissions(&bin_dir, fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("protecting assistant binaries: {error}"))?;
    }
    let installed_binary = bin_dir.join("runx");
    copy_binary(&source_binary, &installed_binary)?;
    copy_binary(&source_worker, &bin_dir.join("runx-js-worker"))?;
    let working_dir = workspace
        .cwd()
        .canonicalize()
        .map_err(|error| format!("resolving assistant working directory: {error}"))?;
    let plist_text = |job_label: &str, action: &str| {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict>\n<key>Label</key><string>{}</string>\n<key>ProgramArguments</key><array><string>{}</string><string>assistant</string><string>{}</string><string>--profile</string><string>{}</string><string>--json</string></array>\n<key>WorkingDirectory</key><string>{}</string>\n<key>StartInterval</key><integer>{}</integer>\n<key>RunAtLoad</key><true/>\n<key>ProcessType</key><string>Background</string>\n</dict></plist>\n",
            escape_xml(job_label),
            escape_xml(&installed_binary.to_string_lossy()),
            action,
            escape_xml(&loaded.path.to_string_lossy()),
            escape_xml(&working_dir.to_string_lossy()),
            loaded.profile.heartbeat_seconds,
        )
    };
    let execute_plist = worker_plist(&plist);
    write_plist(&plist, &plist_text(&label(loaded), "tick"))?;
    write_plist(
        &execute_plist,
        &plist_text(&worker_label(loaded), "execute"),
    )?;
    let domain = format!("gui/{}", uid()?);
    for service in [label(loaded), worker_label(loaded)] {
        let _ = launchctl(&["bootout", &format!("{domain}/{service}")]);
    }
    launchctl(&["bootstrap", &domain, &execute_plist.to_string_lossy()])?;
    if let Err(error) = launchctl(&["bootstrap", &domain, &plist.to_string_lossy()]) {
        let _ = launchctl(&["bootout", &format!("{domain}/{}", worker_label(loaded))]);
        return Err(error);
    }
    Ok(json!({
        "status":"installed",
        "label":label(loaded),
        "interval_seconds":loaded.profile.heartbeat_seconds,
        "plist":plist,
        "execute_plist":execute_plist,
        "binary":installed_binary
    }))
}

pub(super) fn remove_timer(
    loaded: &LoadedProfile,
    workspace: &WorkspaceEnv,
) -> Result<Value, String> {
    if !cfg!(target_os = "macos") {
        return Err("assistant launchd timer is only available on macOS".to_owned());
    }
    let (plist, instance_dir) = paths(loaded, workspace)?;
    let domain = format!("gui/{}", uid()?);
    for (service, path) in [
        (label(loaded), plist.clone()),
        (worker_label(loaded), worker_plist(&plist)),
    ] {
        let _ = launchctl(&["bootout", &format!("{domain}/{service}")]);
        if path.exists() {
            fs::remove_file(&path).map_err(|error| format!("removing assistant timer: {error}"))?;
        }
    }
    if instance_dir.exists() {
        let metadata = fs::symlink_metadata(&instance_dir)
            .map_err(|error| format!("checking assistant installation: {error}"))?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err("assistant installation path is not an owned directory".to_owned());
        }
        let bin = instance_dir.join("bin");
        for name in ["runx", "runx-js-worker"] {
            let path = bin.join(name);
            if path.exists() {
                fs::remove_file(&path)
                    .map_err(|error| format!("removing assistant binary: {error}"))?;
            }
        }
        if bin.exists() {
            fs::remove_dir(&bin).map_err(|error| {
                format!("assistant binary directory contains unexpected files: {error}")
            })?;
        }
        fs::remove_dir(&instance_dir).map_err(|error| {
            format!("assistant installation contains unexpected files: {error}")
        })?;
    }
    Ok(json!({"status":"removed","label":label(loaded)}))
}
