use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use runx_contracts::JsonValue;

use crate::document_input::{DocumentInputSource, read_document_input};

/// The inputs a command line gives a skill, each as text the runner reads by
/// its input's declared type. A name is given once.
#[derive(Debug, Default, PartialEq)]
pub struct CliInputs {
    pub texts: BTreeMap<String, String>,
}

impl CliInputs {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.texts.is_empty()
    }

    /// The inputs the runner `runner` of `skill_path` reads from these: each
    /// text by its input's declared type, never guessed.
    pub(super) fn typed(
        &self,
        skill_path: &Path,
        runner: Option<&str>,
    ) -> Result<BTreeMap<String, JsonValue>, String> {
        if self.texts.is_empty() {
            return Ok(BTreeMap::new());
        }
        runx_runtime::skill_front::runner_inputs_from_text(skill_path, runner, &self.texts)
            .map_err(|error| error.to_string())
    }

    fn insert_text(&mut self, raw_key: &str, value: String) -> Result<(), String> {
        let key = raw_key.trim().to_owned();
        if key.is_empty() {
            return Err("runx skill input key must be non-empty".to_owned());
        }
        if self.texts.contains_key(&key) {
            return Err(format!(
                "runx skill input {key:?} was supplied more than once"
            ));
        }
        self.texts.insert(key, value);
        Ok(())
    }
}

pub(super) fn parse_input_arg(
    args: &[OsString],
    mut index: usize,
    inline_value: Option<&str>,
    inputs: &mut CliInputs,
) -> Result<usize, String> {
    if let Some(value) = inline_value {
        parse_input_assignment(value, None, inputs)?;
        return Ok(index);
    }

    index += 1;
    let key_or_assignment = string_arg(args, index)?;
    if key_or_assignment.contains('=') {
        parse_input_assignment(&key_or_assignment, None, inputs)?;
    } else {
        index += 1;
        parse_input_assignment(&key_or_assignment, Some(string_arg(args, index)?), inputs)?;
    }
    Ok(index)
}

pub(super) fn parse_input_document_arg(
    args: &[OsString],
    mut index: usize,
    inline_value: Option<&str>,
    source: &mut Option<DocumentInputSource>,
) -> Result<usize, String> {
    if source.is_some() {
        return Err("runx skill accepts exactly one --inputs document".to_owned());
    }
    let value = if let Some(value) = inline_value {
        value.to_owned()
    } else {
        index += 1;
        string_arg(args, index)?
    };
    let value = value.trim();
    if value.is_empty() {
        return Err("runx skill --inputs requires a file path or - for stdin".to_owned());
    }
    *source = Some(if value == "-" {
        DocumentInputSource::Stdin
    } else {
        DocumentInputSource::Path(PathBuf::from(value))
    });
    Ok(index)
}

pub(super) fn read_input_document(
    source: &DocumentInputSource,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<BTreeMap<String, JsonValue>, String> {
    let raw = read_document_input(source, env, cwd)
        .map_err(|error| format!("runx skill --inputs could not be read: {error}"))?;
    let value: JsonValue = serde_json::from_str(&raw)
        .map_err(|error| format!("runx skill --inputs contains invalid JSON: {error}"))?;
    value
        .as_object()
        .cloned()
        .ok_or_else(|| "runx skill --inputs must contain one JSON object".to_owned())
}

fn parse_input_assignment(
    key_or_assignment: &str,
    explicit_value: Option<String>,
    inputs: &mut CliInputs,
) -> Result<(), String> {
    match explicit_value {
        Some(value) => inputs.insert_text(key_or_assignment, value),
        None => {
            let (key, value) = key_or_assignment
                .split_once('=')
                .ok_or_else(|| "runx skill -i requires name=value or name value".to_owned())?;
            inputs.insert_text(key, value.to_owned())
        }
    }
}

fn string_arg(args: &[OsString], index: usize) -> Result<String, String> {
    let value = args
        .get(index)
        .ok_or_else(|| "missing value for runx skill argument".to_owned())?;
    value
        .to_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| "runx skill arguments must be UTF-8".to_owned())
}
