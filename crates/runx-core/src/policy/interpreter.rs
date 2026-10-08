use super::posix_basename::posix_basename;

pub(crate) struct InlineInterpreter {
    pub command: String,
    pub trigger: String,
}

pub fn strict_process_tool_inline_code_denial(
    command: Option<&str>,
    args: &[String],
    policy: Option<&super::LocalExecutionPolicy>,
) -> Option<String> {
    if !policy
        .and_then(|policy| policy.strict_process_tool_inline_code)
        .unwrap_or(false)
    {
        return None;
    }
    let interpreter = detect_inline_interpreter(command, args)?;
    Some(format!(
        "process '{}' uses inline code via '{}', which is rejected by strict workspace policy; move the program into a checked-in script and invoke that file instead",
        interpreter.command, interpreter.trigger
    ))
}

pub(crate) fn detect_inline_interpreter(
    command: Option<&str>,
    args: &[String],
) -> Option<InlineInterpreter> {
    let command_name = normalize_executable_name(command?);
    if command_name.is_empty() {
        return None;
    }

    if command_name == "env" {
        let (forwarded_command, forwarded_args) = unwrap_env_command(args)?;
        return detect_inline_interpreter(Some(&forwarded_command), &forwarded_args);
    }

    let lowered_args = args
        .iter()
        .map(|arg| arg.trim().to_owned())
        .collect::<Vec<_>>();

    detect_inline_trigger(&command_name, &lowered_args).map(|trigger| InlineInterpreter {
        command: command_name,
        trigger,
    })
}

fn detect_inline_trigger(command_name: &str, args: &[String]) -> Option<String> {
    if matches!(command_name, "node" | "nodejs" | "bun") {
        return find_exact_arg(args, &["-e", "--eval", "-p", "--print"]);
    }
    if command_name == "deno" {
        return args
            .first()
            .filter(|arg| arg.eq_ignore_ascii_case("eval"))
            .cloned();
    }
    if is_python_like(command_name) {
        return find_exact_arg(args, &["-c"]);
    }
    if matches!(command_name, "ruby" | "perl" | "lua") {
        return find_exact_arg(args, &["-e"]);
    }
    if command_name == "php" {
        return find_exact_arg(args, &["-r"]);
    }
    if matches!(
        command_name,
        "sh" | "bash" | "zsh" | "dash" | "ksh" | "ash" | "fish"
    ) {
        return args.iter().find(|arg| is_shell_c_flag(arg)).cloned();
    }
    if matches!(command_name, "pwsh" | "powershell") {
        return find_exact_arg(args, &["-c", "-command", "-encodedcommand"]);
    }
    if command_name == "cmd" {
        return find_exact_arg(args, &["/c", "/k"]).map(|trigger| trigger.to_ascii_lowercase());
    }
    None
}

fn normalize_executable_name(command: &str) -> String {
    strip_windows_executable_suffix(&posix_basename(command).to_lowercase())
}

fn strip_windows_executable_suffix(command: &str) -> String {
    for suffix in [".exe", ".cmd", ".bat"] {
        if let Some(stripped) = command.strip_suffix(suffix) {
            return stripped.to_owned();
        }
    }
    command.to_owned()
}

fn unwrap_env_command(args: &[String]) -> Option<(String, Vec<String>)> {
    let trimmed_args = args
        .iter()
        .map(|arg| arg.trim())
        .filter(|arg| !arg.is_empty())
        .collect::<Vec<_>>();
    let mut index = 0;

    while trimmed_args
        .get(index)
        .is_some_and(|arg| is_env_assignment(arg))
    {
        index += 1;
    }

    let command = trimmed_args.get(index)?;
    let forwarded_args = trimmed_args[index + 1..]
        .iter()
        .map(|arg| (*arg).to_owned())
        .collect();
    Some(((*command).to_owned(), forwarded_args))
}

fn find_exact_arg(args: &[String], candidates: &[&str]) -> Option<String> {
    args.iter()
        .find(|arg| {
            candidates
                .iter()
                .any(|candidate| arg.eq_ignore_ascii_case(candidate))
        })
        .cloned()
}

fn is_env_assignment(value: &str) -> bool {
    let Some((name, _)) = value.split_once('=') else {
        return false;
    };
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|char| char == '_' || char.is_ascii_alphanumeric())
}

fn is_python_like(command_name: &str) -> bool {
    command_name == "python"
        || command_name == "pypy"
        || command_name
            .strip_prefix("python")
            .is_some_and(is_python_version_suffix)
}

fn is_python_version_suffix(value: &str) -> bool {
    if value.is_empty() {
        return false;
    }
    let parts = value.split('.').collect::<Vec<_>>();
    matches!(parts.as_slice(), [major] if digits_only(major))
        || matches!(parts.as_slice(), [major, minor] if digits_only(major) && digits_only(minor))
}

fn digits_only(value: &str) -> bool {
    !value.is_empty() && value.chars().all(|char| char.is_ascii_digit())
}

fn is_shell_c_flag(value: &str) -> bool {
    let Some(flags) = value.strip_prefix('-') else {
        return false;
    };
    !flags.is_empty()
        && flags.chars().all(|char| char.is_ascii_alphabetic())
        && flags.chars().any(|char| char == 'c')
}

#[cfg(test)]
mod tests {
    use super::detect_inline_interpreter;

    #[test]
    fn unwraps_env_assignments_before_interpreter_detection() {
        let args = vec![
            "PYTHONPATH=.".to_owned(),
            "python3".to_owned(),
            "-c".to_owned(),
        ];

        let detected = detect_inline_interpreter(Some("/usr/bin/env"), &args);

        assert!(detected.is_some_and(|value| value.command == "python3" && value.trigger == "-c"));
    }

    #[test]
    fn strips_windows_executable_suffix_and_detects_node_eval() {
        let args = vec!["-e".to_owned(), "console.log('hi')".to_owned()];

        let detected = detect_inline_interpreter(Some(r"C:\Tools\node.exe"), &args);

        assert!(detected.is_some_and(|value| value.command == "node" && value.trigger == "-e"));
    }

    #[test]
    fn lowercases_the_cmd_trigger() {
        let args = vec!["/C".to_owned(), "echo hi".to_owned()];

        let detected = detect_inline_interpreter(Some("cmd"), &args);

        assert!(detected.is_some_and(|value| value.command == "cmd" && value.trigger == "/c"));
    }
}
