use std::ffi::OsString;

pub fn os_arg<'a>(args: &'a [OsString], index: usize, command: &str) -> Result<&'a str, String> {
    args.get(index)
        .and_then(|arg| arg.to_str())
        .ok_or_else(|| format!("{command} arguments must be UTF-8"))
}

/// Whether a token is a flag: a long `--name`, or `-j`, which every command
/// that prints JSON reads as `--json`.
pub fn is_flag(token: &str) -> bool {
    token.starts_with("--") || token == "-j"
}

pub fn split_flag(token: &str) -> (&str, Option<&str>) {
    token
        .split_once('=')
        .map_or((token, None), |(flag, value)| (flag, Some(value)))
}

pub fn flag_value(
    args: &[OsString],
    index: usize,
    flag: &str,
    inline_value: Option<&str>,
    command: &str,
) -> Result<(String, usize), String> {
    if let Some(value) = inline_value {
        return Ok((value.to_owned(), index + 1));
    }
    let value = os_arg(args, index + 1, command).map_err(|_| format!("{flag} requires a value"))?;
    if value.starts_with("--") {
        return Err(format!("{flag} requires a value"));
    }
    Ok((value.to_owned(), index + 2))
}

pub fn os_flag_value(
    args: &[OsString],
    index: usize,
    flag: &str,
    inline_value: Option<&str>,
) -> Result<(OsString, usize), String> {
    if let Some(value) = inline_value {
        return Ok((OsString::from(value), index + 1));
    }
    let value = args
        .get(index + 1)
        .ok_or_else(|| format!("{flag} requires a value"))?;
    if value.to_str().is_some_and(|value| value.starts_with("--")) {
        return Err(format!("{flag} requires a value"));
    }
    Ok((value.clone(), index + 2))
}

pub fn optional_flag_value(
    args: &[OsString],
    index: usize,
    inline_value: Option<&str>,
    command: &str,
) -> Result<(Option<String>, usize), String> {
    if let Some(value) = inline_value {
        return Ok((Some(value.to_owned()), index + 1));
    }
    let Some(value) = args.get(index + 1).and_then(|arg| arg.to_str()) else {
        return Ok((None, index + 1));
    };
    if value.starts_with('-') {
        return Ok((None, index + 1));
    }
    os_arg(args, index + 1, command)?;
    Ok((Some(value.to_owned()), index + 2))
}

pub fn optional_flag_value_or(
    args: &[OsString],
    index: usize,
    inline_value: Option<&str>,
    default_value: &str,
    command: &str,
) -> Result<(String, usize), String> {
    if let Some(value) = inline_value {
        if value.is_empty() {
            return Ok((default_value.to_owned(), index + 1));
        }
        return Ok((value.to_owned(), index + 1));
    }
    match args.get(index + 1).and_then(|arg| arg.to_str()) {
        Some(value) if !value.starts_with("--") => {
            os_arg(args, index + 1, command)?;
            Ok((value.to_owned(), index + 2))
        }
        _ => Ok((default_value.to_owned(), index + 1)),
    }
}
