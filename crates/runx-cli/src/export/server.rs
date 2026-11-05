//! The local MCP server a host works calling skills on. Its args name every
//! exported skill whose agent makes calls, read back from the managed shims so
//! a partial export keeps what an earlier one registered. Claude Code owns its
//! own config, so its CLI registers the server; Codex reads a documented file.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use runx_runtime::export::parse_export_binding;
use serde::Serialize;

use super::shim::shell_quote;
use super::{
    ExportError, MCP_SERVER_NAME, MCP_SERVER_REF_MARKER, Target, display_path, home_dir, managed,
};

/// Codex's defaults of 10 and 60 seconds are too short here: the server binds
/// every skill it serves when it starts, and a governed call can run a child
/// skill for minutes.
const CODEX_STARTUP_TIMEOUT_SEC: i64 = 60;
const CODEX_TOOL_TIMEOUT_SEC: i64 = 600;

/// The server this export left a host with, and how to register it by hand
/// when the host could not be reached.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ExportedServer {
    pub name: String,
    pub refs: Vec<String>,
    pub registered: bool,
    pub command: String,
}

pub(super) struct ServerExport<'a> {
    pub target: Target,
    pub project: bool,
    pub cwd: &'a Path,
    pub env: &'a BTreeMap<String, String>,
    pub skill_dir: &'a Path,
    pub runx_bin: &'a Path,
}

pub(super) fn register(export: &ServerExport<'_>) -> Result<Option<ExportedServer>, ExportError> {
    let refs = served_refs(export.target, export.skill_dir)?;
    let args = std::iter::once("mcp".to_owned())
        .chain(refs.iter().cloned())
        .collect::<Vec<_>>();
    let registered = match export.target {
        Target::Claude => register_with_claude(export, &refs, &args),
        Target::Codex => {
            write_codex_config(&codex_config_file(export), export.runx_bin, &refs, &args)?;
            true
        }
    };
    if refs.is_empty() {
        return Ok(None);
    }
    Ok(Some(ExportedServer {
        name: MCP_SERVER_NAME.to_owned(),
        registered,
        command: registration_command(export, &args),
        refs,
    }))
}

fn served_refs(target: Target, skill_dir: &Path) -> Result<Vec<String>, ExportError> {
    let entries = match fs::read_dir(skill_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(ExportError::Io {
                context: format!("reading {}", display_path(skill_dir)),
                source,
            });
        }
    };
    let mut refs = BTreeSet::new();
    for entry in entries {
        let entry = entry.map_err(|source| ExportError::Io {
            context: format!("reading {}", display_path(skill_dir)),
            source,
        })?;
        let shim = entry.path().join("SKILL.md");
        if !shim.exists() {
            continue;
        }
        let contents = managed::read_to_string(&shim)?;
        if !contents.contains(MCP_SERVER_REF_MARKER) {
            continue;
        }
        if let Some(binding) = parse_export_binding(&contents)
            && binding.target == target.as_str()
        {
            refs.insert(binding.source.to_owned());
        }
    }
    Ok(refs.into_iter().collect())
}

fn claude_scope(project: bool) -> &'static str {
    if project { "project" } else { "user" }
}

/// Whether Claude Code now has the server. Its CLI refuses an existing name
/// and cannot read an entry back, so a registration replaces what was there,
/// including any environment set on it by hand.
fn register_with_claude(export: &ServerExport<'_>, refs: &[String], args: &[String]) -> bool {
    let Some(claude) = on_path(export.env, "claude") else {
        return false;
    };
    let scope = claude_scope(export.project);
    let host = |host_args: &[&str]| {
        Command::new(&claude)
            .args(host_args)
            .current_dir(export.cwd)
            .env_clear()
            .envs(export.env)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    };
    let removed = host(&["mcp", "remove", MCP_SERVER_NAME, "--scope", scope]);
    if refs.is_empty() {
        return removed;
    }
    let bin = display_path(export.runx_bin);
    let mut add = vec!["mcp", "add", "--scope", scope, MCP_SERVER_NAME, "--", &bin];
    add.extend(args.iter().map(String::as_str));
    host(&add)
}

fn on_path(env: &BTreeMap<String, String>, program: &str) -> Option<PathBuf> {
    std::env::split_paths(env.get("PATH")?)
        .map(|directory| directory.join(program))
        .find(|path| path.is_file())
}

fn codex_config_file(export: &ServerExport<'_>) -> PathBuf {
    if export.project {
        return export.cwd.join(".codex").join("config.toml");
    }
    export
        .env
        .get("CODEX_HOME")
        .map_or_else(
            || home_dir(export.env, export.cwd).join(".codex"),
            PathBuf::from,
        )
        .join("config.toml")
}

/// Set or clear this server's entry. Runx owns its command and args; whatever
/// else the person set on it, and the rest of the file, stay as written.
fn write_codex_config(
    path: &Path,
    runx_bin: &Path,
    refs: &[String],
    args: &[String],
) -> Result<(), ExportError> {
    let existing = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(source) => {
            return Err(ExportError::Io {
                context: format!("reading {}", display_path(path)),
                source,
            });
        }
    };
    let mut config = existing
        .parse::<toml_edit::DocumentMut>()
        .map_err(|error| {
            ExportError::InvalidArgs(format!("{} is not valid TOML: {error}", display_path(path)))
        })?;
    if refs.is_empty() {
        let Some(servers) = config
            .get_mut("mcp_servers")
            .and_then(toml_edit::Item::as_table_like_mut)
        else {
            return Ok(());
        };
        if servers.remove(MCP_SERVER_NAME).is_none() {
            return Ok(());
        }
    } else {
        let servers = config
            .entry("mcp_servers")
            .or_insert_with(|| {
                let mut table = toml_edit::Table::new();
                table.set_implicit(true);
                toml_edit::Item::Table(table)
            })
            .as_table_like_mut()
            .ok_or_else(|| {
                ExportError::InvalidArgs(format!(
                    "{}: mcp_servers is not a table",
                    display_path(path)
                ))
            })?;
        let server = servers
            .entry(MCP_SERVER_NAME)
            .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()))
            .as_table_like_mut()
            .ok_or_else(|| {
                ExportError::InvalidArgs(format!(
                    "{}: mcp_servers.{MCP_SERVER_NAME} is not a table",
                    display_path(path)
                ))
            })?;
        server.insert("command", toml_edit::value(display_path(runx_bin)));
        server.insert(
            "args",
            toml_edit::value(args.iter().collect::<toml_edit::Array>()),
        );
        for (key, seconds) in [
            ("startup_timeout_sec", CODEX_STARTUP_TIMEOUT_SEC),
            ("tool_timeout_sec", CODEX_TOOL_TIMEOUT_SEC),
        ] {
            if !server.contains_key(key) {
                server.insert(key, toml_edit::value(seconds));
            }
        }
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| ExportError::Io {
            context: format!("creating {}", display_path(parent)),
            source,
        })?;
    }
    fs::write(path, config.to_string()).map_err(|source| ExportError::Io {
        context: format!("writing {}", display_path(path)),
        source,
    })
}

fn registration_command(export: &ServerExport<'_>, args: &[String]) -> String {
    let server = std::iter::once(display_path(export.runx_bin))
        .chain(args.iter().cloned())
        .map(|word| shell_quote(&word))
        .collect::<Vec<_>>()
        .join(" ");
    match export.target {
        Target::Claude => format!(
            "claude mcp add --scope {} {MCP_SERVER_NAME} -- {server}",
            claude_scope(export.project)
        ),
        Target::Codex => format!("codex mcp add {MCP_SERVER_NAME} -- {server}"),
    }
}
