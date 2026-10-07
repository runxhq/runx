use std::io::{self, Read, Write};
use std::process::ExitCode;

pub(crate) fn write_stdout(message: &str) -> io::Result<()> {
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    handle.write_all(message.as_bytes())
}

pub(crate) fn write_stdout_code(message: &str, exit_code: u8) -> ExitCode {
    if write_stdout(message).is_ok() {
        ExitCode::from(exit_code)
    } else {
        ExitCode::from(1)
    }
}

pub(crate) fn write_stderr(message: &str) -> io::Result<()> {
    io::stderr().write_all(message.as_bytes())
}

/// A secret piped on stdin, without its trailing newline.
pub(crate) fn read_stdin_secret(what: &str) -> Result<String, String> {
    let mut secret = String::new();
    io::stdin()
        .read_to_string(&mut secret)
        .map_err(|error| format!("reading {what} from stdin: {error}"))?;
    while secret.ends_with(['\r', '\n']) {
        secret.pop();
    }
    if secret.is_empty() {
        return Err(format!("{what} from stdin must not be empty"));
    }
    Ok(secret)
}

pub(crate) fn write_stderr_code(message: &str) -> ExitCode {
    if write_stderr(message).is_ok() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}
