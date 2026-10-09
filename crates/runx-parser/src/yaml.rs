// Module rationale: the QuoteScanner state machine and its
// quote-aware scanners belong next to the supported-subset rules they enforce;
// splitting the scanner from the rules trades clarity for two-file traversal.
use std::collections::HashSet;

use serde::de::DeserializeOwned;

use crate::ParseError;

const DIVERGENT_BOOLISH: &[&str] = &["yes", "no", "on", "off"];
const LEFT_BRACE_BYTE: u8 = b'{';

pub fn parse_yaml_document<T>(source: &str) -> Result<T, ParseError>
where
    T: DeserializeOwned,
{
    assert_yaml_supported_subset("yaml", source)?;
    serde_norway::from_str(source).map_err(|error| ParseError::InvalidYaml {
        field: "yaml".to_owned(),
        message: error.to_string(),
    })
}

pub fn assert_yaml_supported_subset(field: &str, source: &str) -> Result<(), ParseError> {
    let mut block_scalar_indent = None;
    for (line_index, line) in source.lines().enumerate() {
        let line_number = line_index + 1;
        let Some(content) = strip_yaml_comment(line) else {
            continue;
        };
        let trimmed = content.trim();
        if let Some(indent) = block_scalar_indent {
            if trimmed.is_empty() || leading_spaces(content) > indent {
                continue;
            }
            block_scalar_indent = None;
        }
        if trimmed.is_empty() || trimmed.starts_with("---") || trimmed.starts_with("...") {
            continue;
        }
        reject_explicit_mapping_key(field, line_number, trimmed)?;
        reject_embedded_colon_key(field, line_number, trimmed)?;
        reject_colon_space_plain_scalar(field, line_number, content)?;
        block_scalar_indent = block_scalar_indent_after(content).or(block_scalar_indent);
    }
    Ok(())
}

pub fn assert_execution_profile_yaml_subset(field: &str, source: &str) -> Result<(), ParseError> {
    assert_yaml_supported_subset(field, source)?;
    let mut mapping_stack = Vec::new();
    let mut block_scalar_indent = None;
    for (line_index, line) in source.lines().enumerate() {
        let line_number = line_index + 1;
        let Some(content) = strip_yaml_comment(line) else {
            continue;
        };
        let trimmed = content.trim();
        if let Some(indent) = block_scalar_indent {
            if trimmed.is_empty() || leading_spaces(content) > indent {
                continue;
            }
            block_scalar_indent = None;
        }
        if trimmed.is_empty() {
            continue;
        }
        reject_document_marker(field, line_number, trimmed)?;
        reject_yaml_reference_syntax(field, line_number, content)?;
        reject_duplicate_mapping_key(field, line_number, content, &mut mapping_stack)?;
        block_scalar_indent = block_scalar_indent_after(content).or(block_scalar_indent);
    }
    Ok(())
}

/// `false` for scalars YAML would coerce to a non-string (booleans, numbers,
/// dates, special floats), so callers know to keep them quoted.
///
/// # Examples
///
/// ```
/// use runx_parser::yaml::yaml_scalar_subset_allows;
///
/// assert!(yaml_scalar_subset_allows("echo")); // plain string: safe
/// assert!(!yaml_scalar_subset_allows("yes")); // YAML boolean: ambiguous
/// ```
#[must_use]
pub fn yaml_scalar_subset_allows(literal: &str) -> bool {
    let trimmed = literal.trim();
    !is_boolish(trimmed)
        && !is_base_prefixed_number(trimmed)
        && !is_sexagesimal_like(trimmed)
        && !is_date_like(trimmed)
        && !is_special_float(trimmed)
}

pub fn assert_yaml_scalar_subset(field: &str, literal: &str) -> Result<(), ParseError> {
    if yaml_scalar_subset_allows(literal) {
        return Ok(());
    }
    Err(ParseError::UnsupportedScalar {
        field: field.to_owned(),
        literal: literal.to_owned(),
    })
}

fn strip_yaml_comment(line: &str) -> Option<&str> {
    let mut scanner = QuoteScanner::new();
    for (index, char) in line.char_indices() {
        if scanner.is_plain_at(char) && char == '#' && is_comment_start(line, index) {
            return Some(&line[..index]);
        }
        scanner.consume(char);
    }
    Some(line)
}

fn is_comment_start(line: &str, index: usize) -> bool {
    index == 0 || line[..index].ends_with(char::is_whitespace)
}

fn reject_explicit_mapping_key(
    field: &str,
    line_number: usize,
    trimmed: &str,
) -> Result<(), ParseError> {
    if trimmed == "?" || trimmed.starts_with("? ") {
        return Err(ambiguous_yaml(field, line_number, trimmed));
    }
    Ok(())
}

fn reject_embedded_colon_key(
    field: &str,
    line_number: usize,
    trimmed: &str,
) -> Result<(), ParseError> {
    let Some((key, _)) = top_level_plain_key(trimmed) else {
        return Ok(());
    };
    if key.contains(':') {
        return Err(ambiguous_yaml(field, line_number, trimmed));
    }
    Ok(())
}

fn top_level_plain_key(trimmed: &str) -> Option<(&str, usize)> {
    let bytes = trimmed.as_bytes();
    if bytes
        .first()
        .is_some_and(|byte| matches!(*byte, b'-' | b'?' | LEFT_BRACE_BYTE | b'[' | b'"' | b'\''))
    {
        return None;
    }
    let mut scanner = QuoteScanner::new();
    for (index, char) in trimmed.char_indices() {
        if scanner.is_plain_at(char) && char == ':' && is_mapping_delimiter(trimmed, index) {
            return Some((trimmed[..index].trim(), index));
        }
        scanner.consume(char);
    }
    None
}

/// YAML quoted-scalar state machine used by the supported-subset scanners.
///
/// The earlier ad-hoc `previous != '\\'` toggle failed on YAML's double-quote
/// escape rule (`\\` is one escape pair producing a literal `\`; the following
/// byte is not the escape target) and on single-quote escapes (`''` is the
/// escape, not a pair of toggles). Both shapes let `:`-bearing keys past the
/// validator under specific escape patterns. This scanner consumes escape
/// units in one step so the inside/outside-quote signal matches YAML's reader.
#[derive(Clone, Copy)]
enum QuoteState {
    Plain,
    InDouble,
    /// Single-quote scanner saw a `'` and must decide on the next char whether
    /// it was an escape (`''` -> stay in single) or a terminator.
    InSinglePendingApostrophe,
    InSingle,
    /// Double-quote scanner saw a `\` and must consume the next char as the
    /// escape target without inspecting it.
    InDoubleEscape,
}

struct QuoteScanner {
    state: QuoteState,
}

impl QuoteScanner {
    fn new() -> Self {
        Self {
            state: QuoteState::Plain,
        }
    }

    fn is_plain_at(&self, char: char) -> bool {
        // PendingApostrophe means the prior `'` could be either a terminator
        // or the first half of a `''` escape. The current char decides which:
        // another `'` keeps us in the single-quoted scalar; anything else is
        // plain text after the closed scalar.
        match self.state {
            QuoteState::Plain => true,
            QuoteState::InSinglePendingApostrophe => char != '\'',
            QuoteState::InDouble | QuoteState::InDoubleEscape | QuoteState::InSingle => false,
        }
    }

    fn consume(&mut self, char: char) {
        self.state = match self.state {
            QuoteState::Plain => Self::plain_state_after(char),
            QuoteState::InDouble => match char {
                '\\' => QuoteState::InDoubleEscape,
                '"' => QuoteState::Plain,
                _ => QuoteState::InDouble,
            },
            QuoteState::InDoubleEscape => QuoteState::InDouble,
            QuoteState::InSingle => match char {
                '\'' => QuoteState::InSinglePendingApostrophe,
                _ => QuoteState::InSingle,
            },
            // Resolve the prior `'` as either an escape pair (consume `''`
            // and stay in single-quote) or a terminator (now plain, plus
            // route the current char through the Plain transition table).
            QuoteState::InSinglePendingApostrophe => match char {
                '\'' => QuoteState::InSingle,
                _ => Self::plain_state_after(char),
            },
        };
    }

    fn plain_state_after(char: char) -> QuoteState {
        match char {
            '\'' => QuoteState::InSingle,
            '"' => QuoteState::InDouble,
            _ => QuoteState::Plain,
        }
    }
}

fn is_mapping_delimiter(value: &str, index: usize) -> bool {
    value[index + 1..]
        .chars()
        .next()
        .is_none_or(char::is_whitespace)
}

fn reject_colon_space_plain_scalar(
    field: &str,
    line_number: usize,
    content: &str,
) -> Result<(), ParseError> {
    let Some((_, value)) = split_plain_mapping_value(content) else {
        return Ok(());
    };
    if plain_scalar_contains_colon_space(value) {
        return Err(ambiguous_yaml(field, line_number, value.trim()));
    }
    Ok(())
}

fn reject_document_marker(
    field: &str,
    line_number: usize,
    trimmed: &str,
) -> Result<(), ParseError> {
    if trimmed == "---"
        || trimmed == "..."
        || trimmed.starts_with("--- ")
        || trimmed.starts_with("... ")
    {
        return Err(ParseError::InvalidYaml {
            field: field.to_owned(),
            message: format!(
                "YAML document markers are not supported in run.yaml at line {line_number}; use one plain profile document."
            ),
        });
    }
    Ok(())
}

fn reject_yaml_reference_syntax(
    field: &str,
    line_number: usize,
    content: &str,
) -> Result<(), ParseError> {
    for token in [": &", ": *", ": !", "- &", "- *", "- !"] {
        if contains_plain_token(content, token) {
            return Err(ParseError::InvalidYaml {
                field: field.to_owned(),
                message: format!(
                    "YAML anchors, aliases, and tags are not supported in run.yaml at line {line_number}; write the profile explicitly."
                ),
            });
        }
    }
    let trimmed = content.trim_start();
    if trimmed.starts_with(['&', '*', '!']) {
        return Err(ParseError::InvalidYaml {
            field: field.to_owned(),
            message: format!(
                "YAML anchors, aliases, and tags are not supported in run.yaml at line {line_number}; write the profile explicitly."
            ),
        });
    }
    Ok(())
}

fn contains_plain_token(content: &str, token: &str) -> bool {
    let mut scanner = QuoteScanner::new();
    for (index, char) in content.char_indices() {
        if scanner.is_plain_at(char) && content[index..].starts_with(token) {
            return true;
        }
        scanner.consume(char);
    }
    false
}

struct MappingFrame {
    indent: usize,
    keys: HashSet<String>,
}

fn reject_duplicate_mapping_key(
    field: &str,
    line_number: usize,
    content: &str,
    stack: &mut Vec<MappingFrame>,
) -> Result<(), ParseError> {
    let indent = leading_spaces(content);
    let trimmed = content.trim_start();
    let (key_indent, key, sequence_item) = match sequence_item_key(trimmed, indent) {
        Some(value) => value,
        None => {
            let Some(key) = mapping_key(trimmed) else {
                return Ok(());
            };
            (indent, key, false)
        }
    };
    if key == "<<" {
        return Err(ParseError::InvalidYaml {
            field: field.to_owned(),
            message: format!(
                "YAML merge keys are not supported in run.yaml at line {line_number}; write the profile explicitly."
            ),
        });
    }
    if sequence_item {
        while stack.last().is_some_and(|frame| frame.indent >= key_indent) {
            stack.pop();
        }
    } else {
        while stack.last().is_some_and(|frame| frame.indent > key_indent) {
            stack.pop();
        }
    }
    if stack.last().is_none_or(|frame| frame.indent != key_indent) {
        stack.push(MappingFrame {
            indent: key_indent,
            keys: HashSet::new(),
        });
    }
    let Some(frame) = stack.last_mut() else {
        return Err(ParseError::InvalidYaml {
            field: field.to_owned(),
            message: format!(
                "could not track mapping key {key:?} in run.yaml at line {line_number}"
            ),
        });
    };
    if !frame.keys.insert(key.clone()) {
        return Err(ParseError::InvalidYaml {
            field: field.to_owned(),
            message: format!(
                "duplicate mapping key {key:?} in run.yaml at line {line_number}; keep profile keys unique."
            ),
        });
    }
    Ok(())
}

fn sequence_item_key(trimmed: &str, indent: usize) -> Option<(usize, String, bool)> {
    let rest = trimmed.strip_prefix("- ")?;
    let item = rest.trim_start();
    let leading = rest.len() - item.len();
    let key = mapping_key(item)?;
    Some((indent + 2 + leading, key, true))
}

/// The key a mapping line opens, plain or quoted, as YAML reads it: a quoted
/// key and the same plain key are one key.
fn mapping_key(trimmed: &str) -> Option<String> {
    let mut chars = trimmed.char_indices();
    let quote = match trimmed.chars().next() {
        Some(quote @ ('"' | '\'')) => quote,
        _ => return top_level_plain_key(trimmed).map(|(key, _)| key.to_owned()),
    };
    chars.next();
    let mut key = String::new();
    let mut end = None;
    while let Some((index, char)) = chars.next() {
        match (quote, char) {
            ('"', '\\') => key.extend(chars.next().map(|(_, escaped)| escaped)),
            ('\'', '\'') if trimmed[index + 1..].starts_with('\'') => {
                key.push('\'');
                chars.next();
            }
            (quote_char, char) if char == quote_char => {
                end = Some(index + char.len_utf8());
                break;
            }
            (_, char) => key.push(char),
        }
    }
    let rest = trimmed[end?..].trim_start();
    let after = rest.strip_prefix(':')?;
    after
        .chars()
        .next()
        .is_none_or(char::is_whitespace)
        .then_some(key)
}

fn leading_spaces(content: &str) -> usize {
    content.bytes().take_while(|byte| *byte == b' ').count()
}

fn block_scalar_indent_after(content: &str) -> Option<usize> {
    block_scalar_value_candidates(content)
        .iter()
        .any(|value| is_block_scalar_header(value))
        .then(|| leading_spaces(content))
}

fn block_scalar_value_candidates(content: &str) -> Vec<&str> {
    let mut candidates = Vec::new();
    if let Some((_, value)) = split_plain_mapping_value(content) {
        candidates.push(value);
    }
    let trimmed = content.trim_start();
    if let Some(rest) = trimmed.strip_prefix("- ") {
        let item = rest.trim_start();
        candidates.push(item);
        if let Some((_, value)) = split_plain_mapping_value(item) {
            candidates.push(value);
        }
    }
    candidates
}

fn is_block_scalar_header(value: &str) -> bool {
    let trimmed = value.trim();
    let mut chars = trimmed.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !matches!(first, '|' | '>') {
        return false;
    }
    let mut seen_chomp = false;
    let mut seen_indent = false;
    for char in chars {
        if matches!(char, '+' | '-') && !seen_chomp {
            seen_chomp = true;
        } else if char.is_ascii_digit() && !seen_indent {
            seen_indent = true;
        } else {
            return false;
        }
    }
    true
}

fn split_plain_mapping_value(content: &str) -> Option<(&str, &str)> {
    let trimmed = content.trim_start();
    let (key, delimiter_index) = top_level_plain_key(trimmed)?;
    Some((key, &trimmed[delimiter_index + 1..]))
}

// Function rationale: the scalar exemptions and
// quote-aware colon scanner are one validation rule.
fn plain_scalar_contains_colon_space(value: &str) -> bool {
    let trimmed = value.trim_start();
    if trimmed.is_empty()
        || trimmed.starts_with(['"', '\'', '|', '>', '{', '['])
        || trimmed == "null"
        || matches!(trimmed, "true" | "false")
    {
        return false;
    }
    contains_unquoted_colon_space(trimmed)
}

fn contains_unquoted_colon_space(value: &str) -> bool {
    let mut scanner = QuoteScanner::new();
    for (index, char) in value.char_indices() {
        if scanner.is_plain_at(char) && char == ':' && is_mapping_delimiter(value, index) {
            return true;
        }
        scanner.consume(char);
    }
    false
}

fn ambiguous_yaml(field: &str, line_number: usize, literal: &str) -> ParseError {
    ParseError::InvalidYaml {
        field: field.to_owned(),
        message: format!(
            "ambiguous YAML construct at line {line_number}; quote the value or key: {literal}"
        ),
    }
}

fn is_boolish(value: &str) -> bool {
    DIVERGENT_BOOLISH
        .iter()
        .any(|candidate| value.eq_ignore_ascii_case(candidate))
}

fn is_base_prefixed_number(value: &str) -> bool {
    let unsigned = value.strip_prefix(['+', '-']).unwrap_or(value);
    unsigned.starts_with("0x") || unsigned.starts_with("0X") || unsigned.starts_with("0o")
}

fn is_sexagesimal_like(value: &str) -> bool {
    let unsigned = value.strip_prefix(['+', '-']).unwrap_or(value);
    let mut parts = unsigned.split(':');
    let Some(first) = parts.next() else {
        return false;
    };
    first.chars().all(|char| char.is_ascii_digit())
        && parts.clone().count() > 0
        && parts.all(|part| !part.is_empty() && part.chars().all(|char| char.is_ascii_digit()))
}

fn is_date_like(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 10
        && bytes[0..4].iter().all(u8::is_ascii_digit)
        && bytes[4] == b'-'
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[7] == b'-'
        && bytes[8..10].iter().all(u8::is_ascii_digit)
}

fn is_special_float(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        ".nan" | ".inf" | "+.inf" | "-.inf"
    )
}

#[cfg(test)]
mod tests {
    use super::{
        assert_execution_profile_yaml_subset, assert_yaml_scalar_subset,
        assert_yaml_supported_subset, yaml_scalar_subset_allows,
    };

    #[test]
    fn scalar_subset_rejects_divergent_forms() {
        for literal in ["yes", "ON", "0x10", "0o10", "12:34", "2026-05-18", ".nan"] {
            assert!(!yaml_scalar_subset_allows(literal), "{literal}");
        }
    }

    #[test]
    fn scalar_subset_allows_explicit_json_like_scalars() -> Result<(), crate::ParseError> {
        for literal in ["true", "false", "1", "1.5", "plain text", "\"yes\""] {
            assert_yaml_scalar_subset("fixture", literal)?;
        }
        Ok(())
    }

    // Regression cases for the double-quote-escape state machine. The earlier
    // `previous != '\\'` toggle misread `\\` as still-escaped, so the closing
    // `"` was missed and the scanner over-stayed inside quotes, masking a
    // following ambiguous colon. The new state machine consumes `\` plus the
    // next byte as one escape unit and resolves the close correctly.
    #[test]
    fn supported_subset_accepts_backslash_escape_in_double_quote() -> Result<(), crate::ParseError>
    {
        for literal in [
            "key: \"a\\\\b\"",
            "key: \"trailing\\\\\"",
            "key: \"mid\\\\\"",
            "key: \"\\\\\"",
        ] {
            assert_yaml_supported_subset("fixture", literal)?;
        }
        Ok(())
    }

    #[test]
    fn supported_subset_rejects_colon_space_after_closed_double_quote_with_escapes() {
        // The value `plain "escaped\\" trailing: oops` is ambiguous: the
        // quoted region terminates after `escaped\` (the `\\` is one escape
        // unit, then `"` closes), and the trailing plain text contains
        // `trailing: oops` which is a mapping delimiter. The old scanner
        // stayed inside the quote forever because `previous != '\\'` at the
        // close-quote position incorrectly suppressed the toggle, so it
        // missed the trailing colon-space. The state machine correctly
        // exits the quote at the close and flags the colon-space.
        let result =
            assert_yaml_supported_subset("fixture", "key: plain \"escaped\\\\\" trailing: oops");
        assert!(result.is_err(), "expected rejection, got {result:?}");
    }

    #[test]
    fn supported_subset_rejects_explicit_mapping_keys() {
        let result = assert_yaml_supported_subset("fixture", "? >\r 2>-: ");
        assert!(result.is_err(), "expected rejection, got {result:?}");
    }

    #[test]
    fn execution_profile_subset_rejects_yaml_references_and_document_markers() {
        for literal in [
            "---\nskill: example",
            "runners:\n  one:\n    outputs: &shared\n      result: string",
            "runners:\n  one:\n    outputs: *shared",
            "runners:\n  one:\n    runx:\n      <<: *shared",
            "runners:\n  one:\n    type: !custom graph",
        ] {
            let result = assert_execution_profile_yaml_subset("runner_manifest", literal);
            assert!(result.is_err(), "expected rejection, got {result:?}");
        }
    }

    #[test]
    fn execution_profile_subset_rejects_duplicate_keys_but_allows_sequence_reuse() {
        let result =
            assert_execution_profile_yaml_subset("runner_manifest", "skill: one\nskill: two\n");
        assert!(
            result.is_err(),
            "expected duplicate key rejection, got {result:?}"
        );

        let sequence_result = assert_execution_profile_yaml_subset(
            "runner_manifest",
            r#"
skill: demo
runners:
  demo:
    steps:
      - id: first
        tool: one.tool
      - id: second
        tool: two.tool
"#,
        );
        assert!(
            sequence_result.is_ok(),
            "sequence item maps may reuse keys in separate items: {sequence_result:?}"
        );
    }

    #[test]
    fn execution_profile_subset_reads_quoted_keys_as_keys() {
        // A quoted key closes the mapping before it, so its children are its own.
        let nested = assert_execution_profile_yaml_subset(
            "runner_manifest",
            r#"
turns:
  agent.answer.output:
    output: {}
  "answer/0#agent.answer.output":
    output: {}
"#,
        );
        assert!(
            nested.is_ok(),
            "a quoted key opens its own mapping: {nested:?}"
        );

        // A quoted key and the same plain key are one key.
        let duplicate =
            assert_execution_profile_yaml_subset("runner_manifest", "skill: one\n\"skill\": two\n");
        assert!(
            duplicate.is_err(),
            "expected duplicate key rejection, got {duplicate:?}"
        );
    }

    // Regression cases for the single-quote `''` escape. The earlier toggle
    // flipped on every `'`, so `'it''s'` mis-segmented into three scalars and
    // any `:` after byte 4 was treated as still-quoted.
    #[test]
    fn supported_subset_handles_single_quote_double_escape() -> Result<(), crate::ParseError> {
        for literal in ["key: 'it''s'", "key: 'a''b''c'", "key: ''"] {
            assert_yaml_supported_subset("fixture", literal)?;
        }
        Ok(())
    }

    #[test]
    fn supported_subset_keeps_unicode_mapping_delimiter_on_char_boundary()
    -> Result<(), crate::ParseError> {
        assert_yaml_supported_subset("fixture", "\0\0\0'\0\0\0\0\u{8}'|\u{85}:")?;
        Ok(())
    }

    #[test]
    fn supported_subset_rejects_colon_space_after_closed_single_quote_with_escapes() {
        // The value `plain 'it''s' trailing: oops` is ambiguous: the single
        // quote terminates after `it's` (the `''` is one escape unit), and
        // the trailing `trailing: oops` is a mapping delimiter. The old
        // scanner toggled on every `'` so it mis-segmented the quoted run
        // and could leave itself inside an apparent quote when the trailing
        // colon-space appeared. The state machine resolves the `''` escape
        // and flags the trailing colon-space.
        let result = assert_yaml_supported_subset("fixture", "key: plain 'it''s' trailing: oops");
        assert!(result.is_err(), "expected rejection, got {result:?}");
    }
}
