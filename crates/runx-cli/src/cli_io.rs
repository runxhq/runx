use std::io::{self, IsTerminal, Read, Write};
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
        return Err(format!("no {what} was given on stdin"));
    }
    Ok(secret)
}

/// A credential the operator gives for `wanted`: typed at a prompt that does
/// not echo it when stdin is a terminal, else piped whole on stdin.
pub(crate) fn read_secret(wanted: &str) -> Result<String, String> {
    if !io::stdin().is_terminal() {
        return read_stdin_secret(wanted);
    }
    let secret = rpassword::prompt_password(format!("{wanted} (input hidden): "))
        .map_err(|error| format!("reading {wanted}: {error}"))?;
    let secret = secret.trim_end_matches(['\r', '\n']);
    if secret.is_empty() {
        return Err(format!("no {wanted} was given"));
    }
    Ok(secret.to_owned())
}

/// A failure written to stderr, ending the command with its exit code.
pub(crate) fn write_stderr_code(message: &str, exit_code: u8) -> ExitCode {
    let _ignored = write_stderr(message);
    ExitCode::from(exit_code)
}
