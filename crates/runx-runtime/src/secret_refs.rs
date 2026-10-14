//! Secret references: where a secrets manager holds a secret, read through
//! that manager's own CLI with the operator's sign-in to it, every time it is
//! used. runx keeps the reference, never the value, and writes nothing to the
//! manager.
//!
//! Each scheme is one provider: the program it runs, the fixed arguments, the
//! variables that program signs in with, and what binding pins. The program
//! is found outside the workspace, its arguments are never interpreted by a
//! shell, and it runs only for a reference the operator bound.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use runx_contracts::connector::SecretReference;
use serde_json::Value;

use crate::process::{ProcessSpec, run_process};

/// A manager printing more than this is not printing one secret.
const SECRET_OUTPUT_BYTES: usize = 64 * 1024;
/// Long enough for a manager to refresh its own session.
const READ_TIMEOUT: Duration = Duration::from_secs(60);
/// The most of a manager's error a refusal quotes to the operator binding.
const MAX_ERROR_CHARS: usize = 300;

/// Why a reference is read. Binding is the operator at a terminal, shown the
/// manager's own message; a call is recorded, so its refusal carries only
/// what is recognized as safe to keep.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Purpose {
    Bind,
    Call,
}

/// A secret read through its reference, and what fixes where it came from.
pub(crate) struct Read {
    pub value: String,
    /// The exact resource or server the read reached; `None` for a manager
    /// local to the machine.
    pub pinned: Option<String>,
}

impl std::fmt::Debug for Read {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Read")
            .field("pinned", &self.pinned)
            .finish_non_exhaustive()
    }
}

/// Read `reference` now. `pinned` is what binding recorded: a call goes to
/// exactly that resource or server and refuses anything else. Binding passes
/// `None` and records the returned pin.
pub(crate) fn read(
    env: &BTreeMap<String, String>,
    cwd: &Path,
    reference: &SecretReference,
    pinned: Option<&str>,
    purpose: Purpose,
) -> Result<Read, String> {
    // Every part of a reference a manager's CLI reads becomes one of its
    // arguments; none may read as a flag (`vault://secret/-address=...`).
    // A file is read directly, and `op` takes the whole reference as one
    // argument, so their parts may hold any name.
    if !matches!(reference.scheme(), "file" | "op")
        && reference
            .locator()
            .split('/')
            .chain(reference.field())
            .any(|part| part.starts_with('-'))
    {
        return Err(format!(
            "{reference}: no part of a reference may begin with '-'"
        ));
    }
    // A call reads only where binding proved access.
    if purpose == Purpose::Call
        && pinned.is_none()
        && matches!(reference.scheme(), "op" | "aws-sm" | "aws-ps" | "vault")
    {
        return Err(format!(
            "{reference} was bound without a pin; bind it again with --from"
        ));
    }
    let manager = Manager {
        env,
        cwd,
        reference,
        purpose,
    };
    let raw = match reference.scheme() {
        "op" => manager.one_password(pinned)?,
        "aws-sm" => manager.aws_secrets_manager(pinned)?,
        "aws-ps" => manager.aws_parameter_store(pinned)?,
        "vault" => manager.vault(pinned)?,
        "keychain" => manager.keychain()?,
        "file" => file(reference)?,
        scheme => return Err(format!("runx reads no {scheme}:// references")),
    };
    let value = match (reference.field(), raw.selected) {
        (Some(field), false) => select(reference, &raw.value, field)?,
        _ => raw.value,
    };
    if value.trim().is_empty() {
        return Err(format!("{reference} holds an empty secret"));
    }
    Ok(Read {
        value,
        pinned: raw.pinned,
    })
}

/// Whether a value is a reference to a secret rather than the secret: one
/// runx reads, or one another manager's tool resolves.
pub(crate) fn looks_like_reference(value: &str) -> bool {
    value.trim().split_once("://").is_some_and(|(scheme, _)| {
        runx_contracts::connector::SECRET_SCHEMES.contains(&scheme)
            || ["gcp-sm", "azure-kv", "doppler", "infisical", "bws"].contains(&scheme)
    })
}

/// What a manager returned: the value, whether the manager already selected
/// the reference's field, and the pin.
struct Raw {
    value: String,
    selected: bool,
    pinned: Option<String>,
}

/// One field of a JSON secret.
fn select(reference: &SecretReference, raw: &str, field: &str) -> Result<String, String> {
    let object = serde_json::from_str::<Value>(raw)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .ok_or_else(|| format!("{reference} names a field, and the secret is not a JSON object"))?;
    match object.get(field) {
        Some(Value::String(value)) => Ok(value.clone()),
        Some(_) => Err(format!("{reference}: field {field} is not a string")),
        None => Err(format!("{reference}: the secret has no field {field}")),
    }
}

const OP_INSTALL: &str =
    "install the 1Password CLI and sign in (turn on its integration in the 1Password app)";
/// What `op` signs in and finds its accounts with.
const OP_VARIABLES: &[&str] = &["OP_", "XDG_CONFIG_HOME"];

const AWS_INSTALL: &str = "install the AWS CLI and sign in (aws configure, or aws sso login)";

/// The region an ARN names, as the AWS CLI's `--region`: an ARN read from
/// another region's endpoint is not found.
fn arn_region(id: &str) -> Vec<String> {
    id.strip_prefix("arn:")
        .and_then(|rest| rest.split(':').nth(2))
        .filter(|region| !region.is_empty())
        .map(|region| vec!["--region".to_owned(), region.to_owned()])
        .unwrap_or_default()
}

/// One read of one reference, through its manager's CLI.
struct Manager<'a> {
    env: &'a BTreeMap<String, String>,
    cwd: &'a Path,
    reference: &'a SecretReference,
    purpose: Purpose,
}

impl Manager<'_> {
    /// `op://<vault>/<item>[/<section>]/<field>[#field]`: 1Password's own
    /// reference, read by `op` with the operator's sign-in (the desktop app's
    /// unlock, or a service account token). Binding pins the account `op`
    /// signs in to (`OP_ACCOUNT`, else its default), and every read, the
    /// first included, names that account.
    fn one_password(&self, pinned: Option<&str>) -> Result<Raw, String> {
        let reference = self.reference;
        let path = reference.locator().split('?').next().unwrap_or_default();
        if !(3..=4).contains(&path.split('/').count()) || path.split('/').any(str::is_empty) {
            return Err(format!(
                "{reference}: write op://<vault>/<item>[/<section>]/<field>"
            ));
        }
        let account = match pinned {
            Some(account) => account.to_owned(),
            None => {
                let account = json(
                    reference,
                    &self.run(
                        "op",
                        vec![
                            "account".to_owned(),
                            "get".to_owned(),
                            "--format=json".to_owned(),
                        ],
                        OP_VARIABLES,
                        OP_INSTALL,
                    )?,
                )?;
                string(&account, "/id")
                    .ok_or_else(|| format!("{reference}: 1Password named no account"))?
            }
        };
        let value = self.run(
            "op",
            vec![
                "read".to_owned(),
                "--account".to_owned(),
                account.clone(),
                "--no-newline".to_owned(),
                format!("op://{}", reference.locator()),
            ],
            OP_VARIABLES,
            OP_INSTALL,
        )?;
        Ok(Raw {
            value,
            selected: false,
            pinned: Some(account),
        })
    }

    /// `aws-sm://<name or ARN>[#field]`: the secret's ARN pins the account
    /// and region.
    fn aws_secrets_manager(&self, pinned: Option<&str>) -> Result<Raw, String> {
        let reference = self.reference;
        let id = pinned.unwrap_or(reference.locator());
        let mut args = vec![
            "secretsmanager".to_owned(),
            "get-secret-value".to_owned(),
            "--secret-id".to_owned(),
            id.to_owned(),
            "--output".to_owned(),
            "json".to_owned(),
        ];
        args.extend(arn_region(id));
        let response = json(reference, &self.run("aws", args, &["AWS_"], AWS_INSTALL)?)?;
        let arn =
            string(&response, "/ARN").ok_or_else(|| format!("{reference}: AWS returned no ARN"))?;
        let value = string(&response, "/SecretString").ok_or_else(|| {
            format!("{reference} is a binary secret; runx reads secrets stored as text")
        })?;
        Ok(Raw {
            value,
            selected: false,
            pinned: Some(pinned_to(reference, pinned, arn)?),
        })
    }

    /// `aws-ps://<name or ARN>[#field]`: a parameter, decrypted; its ARN pins
    /// the account and region.
    fn aws_parameter_store(&self, pinned: Option<&str>) -> Result<Raw, String> {
        let reference = self.reference;
        let name = pinned.unwrap_or(reference.locator());
        let mut args = vec![
            "ssm".to_owned(),
            "get-parameter".to_owned(),
            "--name".to_owned(),
            name.to_owned(),
            "--with-decryption".to_owned(),
            "--output".to_owned(),
            "json".to_owned(),
        ];
        args.extend(arn_region(name));
        let response = json(reference, &self.run("aws", args, &["AWS_"], AWS_INSTALL)?)?;
        let arn = string(&response, "/Parameter/ARN")
            .ok_or_else(|| format!("{reference}: AWS returned no ARN"))?;
        let value = string(&response, "/Parameter/Value")
            .ok_or_else(|| format!("{reference}: AWS returned no value"))?;
        Ok(Raw {
            value,
            selected: false,
            pinned: Some(pinned_to(reference, pinned, arn)?),
        })
    }

    /// `vault://<mount>/<path>[#field]`: a KV secret. The Vault address, and
    /// namespace if one is set, pin the server. With a field, Vault selects
    /// it itself, whichever KV version the mount is.
    fn vault(&self, pinned: Option<&str>) -> Result<Raw, String> {
        let reference = self.reference;
        let (mount, path) = reference
            .locator()
            .split_once('/')
            .filter(|(mount, path)| !mount.is_empty() && !path.is_empty())
            .ok_or_else(|| format!("{reference} names no path; write vault://<mount>/<path>"))?;
        let server = match pinned {
            Some(server) => server.to_owned(),
            None => vault_server(self.env)
                .ok_or_else(|| format!("{reference} needs VAULT_ADDR set to the Vault server"))?,
        };
        let (address, namespace) = match server.split_once(VAULT_NAMESPACE_SEPARATOR) {
            Some((address, namespace)) => (address, Some(namespace)),
            None => (server.as_str(), None),
        };
        let mut args = vec![
            "kv".to_owned(),
            "get".to_owned(),
            format!("-address={address}"),
        ];
        if let Some(namespace) = namespace {
            args.push(format!("-namespace={namespace}"));
        }
        args.push(format!("-mount={mount}"));
        match reference.field() {
            Some(field) => args.push(format!("-field={field}")),
            None => args.push("-format=json".to_owned()),
        }
        args.push(path.to_owned());
        let output = self.run("vault", args, &["VAULT_"], VAULT_INSTALL)?;
        let value = match reference.field() {
            Some(_) => one_line(output),
            None => {
                // The mount's own version, as `vault kv` reads it: a version 1
                // secret can be shaped exactly like a version 2 response.
                let mut mount_args = vec!["read".to_owned(), format!("-address={address}")];
                if let Some(namespace) = namespace {
                    mount_args.push(format!("-namespace={namespace}"));
                }
                mount_args.extend([
                    "-format=json".to_owned(),
                    format!("sys/internal/ui/mounts/{mount}"),
                ]);
                let mounted = json(
                    reference,
                    &self.run("vault", mount_args, &["VAULT_"], VAULT_INSTALL)?,
                )?;
                let version_two = mounted
                    .pointer("/data/options/version")
                    .and_then(Value::as_str)
                    == Some("2");
                kv_data(reference, &json(reference, &output)?, version_two)?
            }
        };
        Ok(Raw {
            value,
            selected: true,
            pinned: Some(server),
        })
    }

    /// `keychain://<service>/<account>`: a password in the macOS login
    /// keychain, or in the Linux Secret Service. Local to the machine and its
    /// user: the binding proof is made with that user's runx home.
    fn keychain(&self) -> Result<Raw, String> {
        let reference = self.reference;
        let (service, account) = reference
            .locator()
            .rsplit_once('/')
            .filter(|(service, account)| !service.is_empty() && !account.is_empty())
            .ok_or_else(|| {
                format!("{reference} names no account; write keychain://<service>/<account>")
            })?;
        let output = if cfg!(target_os = "macos") {
            self.run(
                "security",
                vec![
                    "find-generic-password".to_owned(),
                    "-s".to_owned(),
                    service.to_owned(),
                    "-a".to_owned(),
                    account.to_owned(),
                    "-w".to_owned(),
                ],
                &[],
                "the macOS security tool ships with the system",
            )?
        } else if cfg!(target_os = "linux") {
            self.run(
                "secret-tool",
                vec![
                    "lookup".to_owned(),
                    "service".to_owned(),
                    service.to_owned(),
                    "account".to_owned(),
                    account.to_owned(),
                ],
                &["DBUS_", "XDG_RUNTIME_DIR"],
                "install secret-tool (libsecret-tools) and unlock the keyring",
            )?
        } else {
            return Err(format!(
                "{reference}: keychain:// reads the macOS keychain or the Linux Secret Service"
            ));
        };
        Ok(Raw {
            value: one_line(output),
            selected: false,
            pinned: None,
        })
    }

    /// Run a manager's CLI, found outside the workspace, with fixed
    /// arguments, the baseline environment and the variables it signs in
    /// with, and return what it printed.
    fn run(
        &self,
        program: &str,
        args: Vec<String>,
        signs_in_with: &[&str],
        install: &str,
    ) -> Result<String, String> {
        let reference = self.reference;
        let workspace = crate::RunxPaths::resolve(self.env, self.cwd)
            .workspace()
            .to_path_buf();
        let not_installed =
            || format!("{reference} is read with {program}, which is not installed; {install}");
        let executable =
            trusted_executable(self.env, &workspace, program).ok_or_else(not_installed)?;
        let mut child_env = crate::execution_environment::process_baseline_environment(self.env);
        child_env.extend(
            self.env
                .iter()
                .filter(|(name, _)| signs_in_with.iter().any(|prefix| name.starts_with(prefix)))
                .map(|(name, value)| (name.clone(), value.clone())),
        );
        let outcome = run_process(
            ProcessSpec::new(
                "secret reference",
                executable.to_string_lossy().into_owned(),
                SECRET_OUTPUT_BYTES,
            )
            .args(args)
            .cwd(workspace)
            .env(child_env)
            .timeout(Some(READ_TIMEOUT)),
        )
        .map_err(|error| match error {
            crate::process::ProcessSupervisorError::Spawn { source, .. }
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                not_installed()
            }
            error => format!("{reference}: {program} could not run: {error}"),
        })?;
        if outcome.timed_out {
            return Err(format!(
                "{reference}: {program} did not answer within {}s",
                READ_TIMEOUT.as_secs()
            ));
        }
        if matches!(outcome.status.code(), Some(126 | 127)) {
            return Err(not_installed());
        }
        if !outcome.status.success() {
            let said = String::from_utf8_lossy(&outcome.stderr.bytes)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            return Err(match self.purpose {
                Purpose::Bind if !said.is_empty() => format!(
                    "{reference} could not be read: {}",
                    said.chars().take(MAX_ERROR_CHARS).collect::<String>()
                ),
                Purpose::Bind => format!("{reference} could not be read ({})", outcome.status),
                Purpose::Call => format!(
                    "{reference} could not be read ({program}: {}); bind it again with --from to see the manager's message",
                    recognized(&said).unwrap_or_else(|| outcome.status.to_string())
                ),
            });
        }
        if outcome.stdout.truncated {
            return Err(format!(
                "{reference}: {program} printed more than one secret"
            ));
        }
        String::from_utf8(outcome.stdout.bytes)
            .map_err(|_| format!("{reference}: {program} printed no text"))
    }
}

/// The AWS error codes a recorded refusal may name: a fixed list, so no
/// text a manager prints reaches a receipt.
const AWS_ERROR_CODES: [&str; 14] = [
    "AccessDenied",
    "AccessDeniedException",
    "DecryptionFailure",
    "ExpiredToken",
    "ExpiredTokenException",
    "InternalServiceError",
    "InvalidClientTokenId",
    "InvalidParameterException",
    "InvalidRequestException",
    "ParameterNotFound",
    "ResourceNotFoundException",
    "ThrottlingException",
    "UnrecognizedClientException",
    "ValidationException",
];

/// What a recorded refusal may say of a manager's error: only forms from a
/// fixed list, never the manager's words.
fn recognized(said: &str) -> Option<String> {
    // AWS: "An error occurred (AccessDeniedException) when calling ...".
    if let Some(code) = said
        .split_once("An error occurred (")
        .and_then(|(_, rest)| rest.split_once(')'))
        .and_then(|(code, _)| AWS_ERROR_CODES.into_iter().find(|known| *known == code))
    {
        return Some(code.to_owned());
    }
    // Vault: "... Code: 503. Errors: ...".
    if let Some(code) = said
        .split_once("Code: ")
        .map(|(_, rest)| {
            rest.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
        })
        .filter(|code| code.len() == 3)
    {
        return Some(format!("HTTP {code}"));
    }
    [
        "Unable to locate credentials",
        "Token has expired",
        "SSO session",
        "Vault is sealed",
        "permission denied",
        "could not be found in the keychain",
        // 1Password: "[ERROR] <time> could not read secret '...': \"x\" isn't
        // an item in the \"v\" vault. ...".
        "isn't a vault",
        "isn't an item",
        "does not have a field",
        "account is not signed in",
        "no account found",
        "authorization prompt dismissed",
    ]
    .into_iter()
    .find(|phrase| said.contains(phrase))
    .map(str::to_owned)
}

const VAULT_INSTALL: &str = "install the Vault CLI and sign in (vault login)";

/// How a pinned Vault server records its namespace.
const VAULT_NAMESPACE_SEPARATOR: &str = " namespace ";

/// The Vault server the environment names, with its namespace.
fn vault_server(env: &BTreeMap<String, String>) -> Option<String> {
    let set = |name: &str| {
        env.get(name)
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
    };
    let address = set("VAULT_ADDR")?;
    Some(match set("VAULT_NAMESPACE") {
        Some(namespace) => format!("{address}{VAULT_NAMESPACE_SEPARATOR}{namespace}"),
        None => address.to_owned(),
    })
}

/// A KV secret's data as one JSON object: under `data.data` on a version 2
/// mount, `data` itself on version 1.
fn kv_data(
    reference: &SecretReference,
    response: &Value,
    version_two: bool,
) -> Result<String, String> {
    let data = if version_two {
        response.pointer("/data/data")
    } else {
        response.pointer("/data")
    }
    .filter(|data| data.is_object())
    .ok_or_else(|| format!("{reference}: Vault returned no secret data"))?;
    Ok(data.to_string())
}

/// The manager's executable: the first match on an absolute `PATH` entry
/// outside the workspace, so a project cannot stand in for the manager and
/// receive its sign-in.
fn trusted_executable(
    env: &BTreeMap<String, String>,
    workspace: &Path,
    program: &str,
) -> Option<PathBuf> {
    let workspace = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    std::env::split_paths(env.get("PATH")?)
        .filter(|entry| entry.is_absolute())
        .flat_map(|entry| executable_names(env, program).map(move |name| entry.join(name)))
        // The file itself, symlinks resolved, must sit outside the workspace:
        // a link on a trusted entry could point back into the project.
        .filter_map(|candidate| candidate.canonicalize().ok())
        .filter(|resolved| !resolved.starts_with(&workspace))
        .find(|resolved| is_executable(resolved))
}

/// The names `program` is installed as: itself, and on Windows with each
/// executable extension `PATHEXT` lists.
fn executable_names(env: &BTreeMap<String, String>, program: &str) -> impl Iterator<Item = String> {
    let extensions = if cfg!(windows) {
        env.get("PATHEXT")
            .map_or(".EXE;.CMD;.BAT;.COM", String::as_str)
            .split(';')
            .filter(|extension| !extension.is_empty())
            .map(|extension| format!("{program}{}", extension.to_ascii_lowercase()))
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    std::iter::once(program.to_owned()).chain(extensions)
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

/// `file:///<absolute path>[#field]`: a mounted secret (Docker, Kubernetes,
/// systemd credentials). The path is fixed by the reference.
fn file(reference: &SecretReference) -> Result<Raw, String> {
    let path = Path::new(reference.locator());
    if !path.is_absolute() {
        return Err(format!(
            "{reference} must name an absolute path, file:///..."
        ));
    }
    let cannot = |error: std::io::Error| format!("{reference} cannot be read: {error}");
    let opened = open_without_blocking(path).map_err(cannot)?;
    // The opened file itself must be a regular file: a FIFO or a device
    // swapped in by a symlink could block or never end.
    if !opened.metadata().map_err(cannot)?.is_file() {
        return Err(format!("{reference} is not a secret file"));
    }
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(
        &mut std::io::Read::take(opened, SECRET_OUTPUT_BYTES as u64 + 1),
        &mut bytes,
    )
    .map_err(cannot)?;
    // Some files report no size (procfs), so the bound is on what is read.
    if bytes.len() > SECRET_OUTPUT_BYTES {
        return Err(format!("{reference} is not a secret file"));
    }
    let text = String::from_utf8(bytes).map_err(|_| format!("{reference} is not text"))?;
    Ok(Raw {
        value: one_line(text),
        selected: false,
        pinned: None,
    })
}

#[cfg(unix)]
fn open_without_blocking(path: &Path) -> std::io::Result<std::fs::File> {
    use rustix::fs::{Mode, OFlags, open};
    open(
        path,
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map(std::fs::File::from)
    .map_err(std::io::Error::from)
}

#[cfg(not(unix))]
fn open_without_blocking(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::File::open(path)
}

/// A secret printed or stored with one trailing newline.
fn one_line(mut text: String) -> String {
    if text.ends_with('\n') {
        text.pop();
        if text.ends_with('\r') {
            text.pop();
        }
    }
    text
}

/// A later read must reach what binding pinned.
fn pinned_to(
    reference: &SecretReference,
    pinned: Option<&str>,
    reached: String,
) -> Result<String, String> {
    match pinned {
        Some(pinned) if pinned != reached => Err(format!(
            "{reference} was bound to {pinned} and now reads {reached}; bind it again to accept that"
        )),
        _ => Ok(reached),
    }
}

fn json(reference: &SecretReference, output: &str) -> Result<Value, String> {
    serde_json::from_str(output).map_err(|_| format!("{reference}: the manager printed no JSON"))
}

fn string(value: &Value, pointer: &str) -> Option<String> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(str::to_owned)
}

#[cfg(all(test, unix))]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    use runx_contracts::connector::SecretReference;

    use super::{Purpose, looks_like_reference, read};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// A manager CLI that records its arguments, one per line, and prints
    /// `output`, or fails with `error` on stderr.
    fn fake(bin: &Path, program: &str, output: &str, error: Option<&str>) -> TestResult {
        fs::create_dir_all(bin)?;
        let log = bin.join(format!("{program}.argv"));
        let script = match error {
            Some(error) => format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" >> '{}'\necho '{error}' >&2\nexit 1\n",
                log.display()
            ),
            None => format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" >> '{}'\n/bin/cat <<'JSON'\n{output}\nJSON\n",
                log.display()
            ),
        };
        let path = bin.join(program);
        fs::write(&path, script)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
        Ok(())
    }

    fn argv(bin: &Path, program: &str) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        Ok(fs::read_to_string(bin.join(format!("{program}.argv")))?
            .lines()
            .map(str::to_owned)
            .collect())
    }

    /// A workspace, and a directory of manager CLIs outside it.
    struct Machine {
        _workspace: tempfile::TempDir,
        _tools: tempfile::TempDir,
        workspace: PathBuf,
        bin: PathBuf,
    }

    fn machine() -> Result<Machine, Box<dyn std::error::Error>> {
        let workspace = tempfile::tempdir()?;
        let tools = tempfile::tempdir()?;
        Ok(Machine {
            workspace: workspace.path().to_path_buf(),
            bin: tools.path().join("bin"),
            _workspace: workspace,
            _tools: tools,
        })
    }

    fn env(bin: &Path, extra: &[(&str, &str)]) -> BTreeMap<String, String> {
        let mut env = BTreeMap::from([
            ("PATH".to_owned(), bin.display().to_string()),
            ("AWS_PROFILE".to_owned(), "team".to_owned()),
            ("UNRELATED_SECRET".to_owned(), "never-passed".to_owned()),
        ]);
        env.extend(
            extra
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned())),
        );
        env
    }

    fn env_at(bin: &Path) -> BTreeMap<String, String> {
        env(bin, &[])
    }

    fn reference(value: &str) -> Result<SecretReference, String> {
        SecretReference::new(value).ok_or_else(|| format!("{value} is not a reference"))
    }

    const ARN: &str = "arn:aws:secretsmanager:eu-west-2:111122223333:secret:team/keys-AbCdEf";

    #[test]
    fn aws_secrets_manager_binds_by_name_then_reads_the_pinned_arn_in_its_region() -> TestResult {
        let m = machine()?;
        fake(
            &m.bin,
            "aws",
            &format!(r#"{{"ARN":"{ARN}","SecretString":"{{\"TOKEN\":\"from-aws\"}}"}}"#),
            None,
        )?;
        let env = env(&m.bin, &[]);
        let reference = reference("aws-sm://team/keys#TOKEN")?;

        let bound = read(&env, &m.workspace, &reference, None, Purpose::Bind)?;
        assert_eq!(bound.value, "from-aws");
        assert_eq!(bound.pinned.as_deref(), Some(ARN));
        assert_eq!(
            argv(&m.bin, "aws")?,
            [
                "secretsmanager",
                "get-secret-value",
                "--secret-id",
                "team/keys",
                "--output",
                "json"
            ]
        );

        fs::remove_file(m.bin.join("aws.argv"))?;
        read(&env, &m.workspace, &reference, Some(ARN), Purpose::Call)?;
        assert_eq!(
            argv(&m.bin, "aws")?,
            [
                "secretsmanager",
                "get-secret-value",
                "--secret-id",
                ARN,
                "--output",
                "json",
                "--region",
                "eu-west-2"
            ]
        );
        Ok(())
    }

    #[test]
    fn a_read_that_reaches_another_resource_than_binding_pinned_is_refused() -> TestResult {
        let m = machine()?;
        let other = "arn:aws:secretsmanager:eu-west-2:999999999999:secret:team/keys-ZzZzZz";
        fake(
            &m.bin,
            "aws",
            &format!(r#"{{"ARN":"{other}","SecretString":"from-another-account"}}"#),
            None,
        )?;
        let refused = read(
            &env(&m.bin, &[]),
            &m.workspace,
            &reference("aws-sm://team/keys")?,
            Some(ARN),
            Purpose::Call,
        )
        .err()
        .ok_or("another account's secret was read")?;
        assert!(
            refused.contains(&format!("was bound to {ARN} and now reads {other}")),
            "{refused}"
        );
        Ok(())
    }

    #[test]
    fn a_call_reads_only_where_binding_pinned() -> TestResult {
        let m = machine()?;
        fake(&m.bin, "aws", "{}", None)?;
        fake(&m.bin, "vault", "{}", None)?;
        let env = env(&m.bin, &[("VAULT_ADDR", "https://vault.team:8200")]);
        for value in [
            "aws-sm://team/keys",
            "aws-ps:///team/token",
            "vault://secret/team/keys",
        ] {
            let refused = read(&env, &m.workspace, &reference(value)?, None, Purpose::Call)
                .err()
                .ok_or("an unpinned call was read")?;
            assert!(refused.contains("was bound without a pin"), "{refused}");
        }
        assert!(!m.bin.join("aws.argv").exists() && !m.bin.join("vault.argv").exists());
        Ok(())
    }

    #[test]
    fn parameter_store_decrypts_and_pins_the_parameter_arn() -> TestResult {
        let m = machine()?;
        let arn = "arn:aws:ssm:us-east-1:111122223333:parameter/team/token";
        fake(
            &m.bin,
            "aws",
            &format!(
                r#"{{"Parameter":{{"ARN":"{arn}","Value":"from-ssm","Type":"SecureString"}}}}"#
            ),
            None,
        )?;
        let read = read(
            &env(&m.bin, &[]),
            &m.workspace,
            &reference("aws-ps:///team/token")?,
            None,
            Purpose::Bind,
        )?;
        assert_eq!(
            (read.value.as_str(), read.pinned.as_deref()),
            ("from-ssm", Some(arn))
        );
        assert_eq!(
            argv(&m.bin, "aws")?,
            [
                "ssm",
                "get-parameter",
                "--name",
                "/team/token",
                "--with-decryption",
                "--output",
                "json"
            ]
        );
        Ok(())
    }

    #[test]
    fn vault_selects_a_field_itself_from_the_pinned_server() -> TestResult {
        let m = machine()?;
        fake(&m.bin, "vault", "from-vault", None)?;
        let env = env(
            &m.bin,
            &[
                ("VAULT_ADDR", "https://vault.team:8200"),
                ("VAULT_TOKEN", "t"),
            ],
        );
        let reference = reference("vault://secret/team/keys#TOKEN")?;
        let bound = read(&env, &m.workspace, &reference, None, Purpose::Bind)?;
        assert_eq!(bound.value, "from-vault");
        assert_eq!(bound.pinned.as_deref(), Some("https://vault.team:8200"));

        // A later read goes to the pinned server, whatever VAULT_ADDR says now.
        fs::remove_file(m.bin.join("vault.argv"))?;
        let mut moved = env.clone();
        moved.insert("VAULT_ADDR".to_owned(), "https://elsewhere:8200".to_owned());
        read(
            &moved,
            &m.workspace,
            &reference,
            Some("https://vault.team:8200"),
            Purpose::Call,
        )?;
        assert_eq!(
            argv(&m.bin, "vault")?,
            [
                "kv",
                "get",
                "-address=https://vault.team:8200",
                "-mount=secret",
                "-field=TOKEN",
                "team/keys"
            ]
        );
        Ok(())
    }

    /// A `vault` that answers the mount query with `version` and a KV read
    /// with `secret`.
    fn fake_vault(bin: &Path, version: &str, secret: &str) -> TestResult {
        fs::create_dir_all(bin)?;
        let path = bin.join("vault");
        fs::write(
            &path,
            format!(
                "#!/bin/sh\nif [ \"$1\" = read ]; then\n/bin/cat <<'JSON'\n{{\"data\":{{\"options\":{{\"version\":\"{version}\"}}}}}}\nJSON\nelse\n/bin/cat <<'JSON'\n{secret}\nJSON\nfi\n"
            ),
        )?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
        Ok(())
    }

    #[test]
    fn vault_reads_a_whole_secret_by_its_mounts_own_version() -> TestResult {
        let m = machine()?;
        let env = env(&m.bin, &[("VAULT_ADDR", "https://vault.team:8200")]);
        let whole = reference("vault://kv/team/keys")?;
        // A version 1 secret shaped exactly like a version 2 response keeps
        // all of its own fields.
        let shaped = r#"{"data":{"data":{"TOKEN":"inner"},"metadata":{"version":3,"created_time":"2026-10-14T00:00:00Z"},"outer":"kept"}}"#;
        for (version, expected) in [
            ("2", serde_json::json!({"TOKEN": "inner"})),
            (
                "1",
                serde_json::json!({
                    "data": {"TOKEN": "inner"},
                    "metadata": {"version": 3, "created_time": "2026-10-14T00:00:00Z"},
                    "outer": "kept"
                }),
            ),
        ] {
            fake_vault(&m.bin, version, shaped)?;
            let read = read(&env, &m.workspace, &whole, None, Purpose::Bind)?;
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&read.value)?,
                expected
            );
        }
        Ok(())
    }

    #[test]
    fn a_link_on_a_trusted_path_that_points_into_the_workspace_is_never_run() -> TestResult {
        let m = machine()?;
        let planted = m.workspace.join("tools");
        fake(
            &planted,
            "aws",
            &format!(r#"{{"ARN":"{ARN}","SecretString":"x"}}"#),
            None,
        )?;
        fs::create_dir_all(&m.bin)?;
        std::os::unix::fs::symlink(planted.join("aws"), m.bin.join("aws"))?;
        let refused = read(
            &env(&m.bin, &[]),
            &m.workspace,
            &reference("aws-sm://team/keys")?,
            None,
            Purpose::Bind,
        )
        .err()
        .ok_or("a link into the project was run")?;
        assert!(refused.contains("which is not installed"), "{refused}");
        assert!(!planted.join("aws.argv").exists());
        Ok(())
    }

    #[test]
    fn a_recorded_refusal_names_only_a_known_aws_code() -> TestResult {
        let m = machine()?;
        fake(
            &m.bin,
            "aws",
            "",
            Some("An error occurred (sekretTokenValue123) when calling GetSecretValue"),
        )?;
        let called = read(
            &env(&m.bin, &[]),
            &m.workspace,
            &reference("aws-sm://team/keys")?,
            Some(ARN),
            Purpose::Call,
        )
        .err()
        .ok_or("a failed call passed")?;
        assert!(!called.contains("sekretTokenValue123"), "{called}");
        assert!(called.contains("exit status: 1"), "{called}");
        Ok(())
    }

    #[test]
    fn the_manager_gets_its_own_variables_and_nothing_else() -> TestResult {
        let m = machine()?;
        fs::create_dir_all(&m.bin)?;
        let seen = m.bin.join("env");
        let path = m.bin.join("aws");
        fs::write(
            &path,
            format!(
                "#!/bin/sh\n/usr/bin/env > '{}'\necho '{{\"ARN\":\"{ARN}\",\"SecretString\":\"x\"}}'\n",
                seen.display()
            ),
        )?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
        read(
            &env(&m.bin, &[]),
            &m.workspace,
            &reference("aws-sm://team/keys")?,
            None,
            Purpose::Bind,
        )?;
        let seen = fs::read_to_string(seen)?;
        assert!(seen.contains("AWS_PROFILE=team"), "{seen}");
        assert!(!seen.contains("UNRELATED_SECRET"), "{seen}");
        Ok(())
    }

    #[test]
    fn a_manager_inside_the_workspace_is_never_run() -> TestResult {
        let m = machine()?;
        let planted = m.workspace.join("bin");
        fake(
            &planted,
            "aws",
            &format!(r#"{{"ARN":"{ARN}","SecretString":"x"}}"#),
            None,
        )?;
        let mut env = env(&planted, &[]);
        env.insert("PATH".to_owned(), format!("bin:{}", planted.display()));
        let refused = read(
            &env,
            &m.workspace,
            &reference("aws-sm://team/keys")?,
            None,
            Purpose::Bind,
        )
        .err()
        .ok_or("a project's aws was run")?;
        assert!(refused.contains("which is not installed"), "{refused}");
        assert!(!planted.join("aws.argv").exists());
        Ok(())
    }

    #[test]
    fn a_recorded_refusal_keeps_only_what_is_known_to_hold_no_secret() -> TestResult {
        let m = machine()?;
        let said = "token=leaked-sentinel An error occurred (AccessDeniedException) when calling GetSecretValue";
        fake(&m.bin, "aws", "", Some(said))?;
        let env = env(&m.bin, &[]);
        let reference = reference("aws-sm://team/keys")?;

        let called = read(&env, &m.workspace, &reference, Some(ARN), Purpose::Call)
            .err()
            .ok_or("a failed call passed")?;
        assert!(called.contains("(aws: AccessDeniedException)"), "{called}");
        assert!(!called.contains("leaked-sentinel"), "{called}");

        let bound = read(&env, &m.workspace, &reference, None, Purpose::Bind)
            .err()
            .ok_or("a failed bind passed")?;
        assert_eq!(
            bound,
            format!("aws-sm://team/keys could not be read: {said}")
        );

        let empty = m.bin.with_file_name("empty");
        fs::create_dir_all(&empty)?;
        let missing = read(
            &env_at(&empty),
            &m.workspace,
            &reference,
            None,
            Purpose::Bind,
        )
        .err()
        .ok_or("a missing CLI passed")?;
        assert!(
            missing.starts_with("aws-sm://team/keys is read with aws, which is not installed"),
            "{missing}"
        );
        Ok(())
    }

    #[test]
    fn a_manager_printing_more_than_one_secret_is_refused() -> TestResult {
        let m = machine()?;
        fake(&m.bin, "aws", &"x".repeat(70 * 1024), None)?;
        let refused = read(
            &env(&m.bin, &[]),
            &m.workspace,
            &reference("aws-sm://team/keys")?,
            None,
            Purpose::Bind,
        )
        .err()
        .ok_or("an oversized answer was read")?;
        assert!(
            refused.contains("printed more than one secret"),
            "{refused}"
        );
        Ok(())
    }

    #[test]
    fn keychain_asks_the_platform_store_for_one_service_and_account() -> TestResult {
        let m = machine()?;
        let program = if cfg!(target_os = "macos") {
            "security"
        } else {
            "secret-tool"
        };
        fake(&m.bin, program, "from-keychain", None)?;
        let read = read(
            &env(&m.bin, &[]),
            &m.workspace,
            &reference("keychain://team/work")?,
            None,
            Purpose::Bind,
        )?;
        assert_eq!((read.value.as_str(), read.pinned), ("from-keychain", None));
        let expected: &[&str] = if cfg!(target_os = "macos") {
            &["find-generic-password", "-s", "team", "-a", "work", "-w"]
        } else {
            &["lookup", "service", "team", "account", "work"]
        };
        assert_eq!(argv(&m.bin, program)?, expected);
        Ok(())
    }

    #[test]
    fn no_part_of_a_reference_reaches_a_manager_as_a_flag() -> TestResult {
        let m = machine()?;
        fake(&m.bin, "vault", "{}", None)?;
        let env = env(&m.bin, &[("VAULT_ADDR", "https://vault.team:8200")]);
        for value in [
            "vault://secret/-address=https://evil",
            "vault://-mount/x",
            "aws-sm://--endpoint-url=https://evil",
            "keychain://service/-a",
            "aws-sm://team/keys#-x",
        ] {
            let refused = read(&env, &m.workspace, &reference(value)?, None, Purpose::Bind)
                .err()
                .ok_or("a flag-shaped part was passed")?;
            assert!(refused.contains("may begin with '-'"), "{refused}");
        }
        assert!(!m.bin.join("vault.argv").exists(), "the manager ran");
        Ok(())
    }

    #[test]
    fn a_file_reference_reads_one_regular_file_and_nothing_else() -> TestResult {
        let m = machine()?;
        // A file is read directly: a name beginning with '-' is no flag.
        let secret = m.workspace.join("-mounts").join("token");
        fs::create_dir_all(m.workspace.join("-mounts"))?;
        fs::write(&secret, "from-a-mount\n")?;
        let env = BTreeMap::new();
        let at = |path: &Path| reference(&format!("file://{}", path.display()));
        let read_file = read(&env, &m.workspace, &at(&secret)?, None, Purpose::Call)?;
        assert_eq!(
            (read_file.value.as_str(), read_file.pinned),
            ("from-a-mount", None)
        );
        assert!(
            read(
                &env,
                &m.workspace,
                &reference("file://relative/token")?,
                None,
                Purpose::Call
            )
            .err()
            .is_some_and(|error| error.contains("must name an absolute path"))
        );

        let large = m.workspace.join("large");
        fs::write(&large, "x".repeat(70 * 1024))?;
        let fifo = m.workspace.join("fifo");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&fifo)
                .status()?
                .success()
        );
        for path in [&large, &fifo, &m.workspace] {
            let refused = read(&env, &m.workspace, &at(path)?, None, Purpose::Call)
                .err()
                .ok_or("a non-secret file was read")?;
            assert!(refused.contains("is not a secret file"), "{refused}");
        }
        Ok(())
    }

    #[test]
    fn a_field_selects_from_a_json_secret_and_names_what_is_missing() -> TestResult {
        let m = machine()?;
        let secret = m.workspace.join("keys.json");
        fs::write(&secret, r#"{"TOKEN":"t","PORT":443}"#)?;
        let env = BTreeMap::new();
        let at = |field: &str| reference(&format!("file://{}#{field}", secret.display()));
        assert_eq!(
            read(&env, &m.workspace, &at("TOKEN")?, None, Purpose::Call)?.value,
            "t"
        );
        for (field, says) in [
            ("MISSING", "has no field MISSING"),
            ("PORT", "field PORT is not a string"),
        ] {
            let refused = read(&env, &m.workspace, &at(field)?, None, Purpose::Call)
                .err()
                .ok_or("a bad field passed")?;
            assert!(refused.contains(says), "{refused}");
        }
        Ok(())
    }

    /// An `op` that records its arguments, answers `account get` for one
    /// account, and reads `secret` as `op read --no-newline` prints it.
    fn fake_op(bin: &Path, secret: &str) -> TestResult {
        fs::create_dir_all(bin)?;
        let path = bin.join("op");
        fs::write(
            &path,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" >> '{}'\nif [ \"$1\" = account ]; then\n/bin/cat <<'JSON'\n{{\"id\":\"ACCOUNT1\",\"name\":\"Team\",\"domain\":\"team\",\"type\":\"BUSINESS\",\"state\":\"ACTIVE\"}}\nJSON\nelse\nprintf '%s' '{secret}'\nfi\n",
                bin.join("op.argv").display()
            ),
        )?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
        Ok(())
    }

    #[test]
    fn one_password_pins_the_signed_in_account_and_names_it_to_every_read() -> TestResult {
        let m = machine()?;
        fake_op(&m.bin, "from-1password")?;
        let env = env(&m.bin, &[("OP_ACCOUNT", "team")]);
        let reference = reference("op://Private/Apollo API/credential")?;

        let bound = read(&env, &m.workspace, &reference, None, Purpose::Bind)?;
        assert_eq!(bound.value, "from-1password");
        assert_eq!(bound.pinned.as_deref(), Some("ACCOUNT1"));
        assert_eq!(
            argv(&m.bin, "op")?,
            [
                "account",
                "get",
                "--format=json",
                "read",
                "--account",
                "ACCOUNT1",
                "--no-newline",
                "op://Private/Apollo API/credential"
            ]
        );

        // A later read names the pinned account, whatever OP_ACCOUNT says now.
        fs::remove_file(m.bin.join("op.argv"))?;
        let mut moved = env.clone();
        moved.insert("OP_ACCOUNT".to_owned(), "elsewhere".to_owned());
        read(
            &moved,
            &m.workspace,
            &reference,
            Some("ACCOUNT1"),
            Purpose::Call,
        )?;
        assert_eq!(
            argv(&m.bin, "op")?,
            [
                "read",
                "--account",
                "ACCOUNT1",
                "--no-newline",
                "op://Private/Apollo API/credential"
            ]
        );
        Ok(())
    }

    #[test]
    fn a_one_password_reference_names_a_field_and_a_call_needs_its_account() -> TestResult {
        let m = machine()?;
        fake_op(&m.bin, "never-read")?;
        let env = env(&m.bin, &[]);
        for value in [
            "op://Private/Apollo API",
            "op://Private//credential",
            "op://Private/Apollo/section/sub/credential",
        ] {
            let refused = read(&env, &m.workspace, &reference(value)?, None, Purpose::Bind)
                .err()
                .ok_or("a reference naming no field was read")?;
            assert!(refused.contains("write op://<vault>/<item>"), "{refused}");
        }
        let refused = read(
            &env,
            &m.workspace,
            &reference("op://Private/Apollo API/credential")?,
            None,
            Purpose::Call,
        )
        .err()
        .ok_or("an unpinned call was read")?;
        assert!(refused.contains("was bound without a pin"), "{refused}");
        assert!(!m.bin.join("op.argv").exists(), "op ran");
        Ok(())
    }

    #[test]
    fn a_recorded_one_password_refusal_names_only_a_known_form() -> TestResult {
        let m = machine()?;
        let said = "[ERROR] 2026/10/14 20:00:00 \"leaked-sentinel\" isn't an item in the \"Private\" vault.";
        fs::create_dir_all(&m.bin)?;
        let op = m.bin.join("op");
        fs::write(
            &op,
            format!("#!/bin/sh\n/bin/cat >&2 <<'ERR'\n{said}\nERR\nexit 1\n"),
        )?;
        fs::set_permissions(&op, fs::Permissions::from_mode(0o755))?;
        let called = read(
            &env(&m.bin, &[]),
            &m.workspace,
            &reference("op://Private/leaked-sentinel/credential")?,
            Some("ACCOUNT1"),
            Purpose::Call,
        )
        .err()
        .ok_or("a failed call passed")?;
        assert!(called.contains("(op: isn't an item)"), "{called}");
        assert!(!called.contains("[ERROR]"), "{called}");
        Ok(())
    }

    #[test]
    fn values_that_are_references_are_recognized_as_references() {
        for value in [
            "op://dev/exa/credential",
            "aws-sm://team/keys",
            "vault://secret/x",
            " file:///run/secrets/x",
        ] {
            assert!(looks_like_reference(value), "{value}");
        }
        for value in ["sk-live-123", "https://hooks.example/abc", "ghp_abc"] {
            assert!(!looks_like_reference(value), "{value}");
        }
    }
}
