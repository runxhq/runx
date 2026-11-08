use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::OnceLock;

use regex::Regex;
use runx_contracts::{JsonObject, JsonValue};

use crate::ParseError;

use super::RawSkillIr;

pub fn parse_skill_markdown(markdown: &str) -> Result<RawSkillIr, ParseError> {
    static SKILL_FRONTMATTER_PATTERN: OnceLock<Result<Regex, String>> = OnceLock::new();
    let pattern = match SKILL_FRONTMATTER_PATTERN.get_or_init(|| {
        Regex::new(r"(?s)^---\r?\n(.*?)\r?\n---\r?\n?(.*)$").map_err(|error| error.to_string())
    }) {
        Ok(pattern) => pattern,
        Err(message) => {
            return Err(ParseError::InvalidDocument {
                field: "skill".to_owned(),
                message: message.clone(),
            });
        }
    };
    let Some(captures) = pattern.captures(markdown) else {
        return Err(ParseError::InvalidDocument {
            field: "skill".to_owned(),
            message: "Skill markdown must start with YAML frontmatter delimited by ---.".to_owned(),
        });
    };
    let raw_frontmatter = capture_string(&captures, 1)?;
    let body = capture_string(&captures, 2)?;
    let frontmatter = parse_yaml_object(
        &raw_frontmatter,
        "Skill frontmatter must parse to an object.",
    )?;
    Ok(RawSkillIr {
        frontmatter,
        raw_frontmatter,
        body,
    })
}

const AGENT_TASK_HEADING: &str = "## Agent task: ";

/// A manual's `## Agent task: <name>` sections, outside code, by name, each
/// as the bytes it spans. A section carries its task's own instructions,
/// persona included, and runs from its heading to the next heading of level
/// one or two, so its own `###` headings stay inside it.
///
/// # Errors
///
/// Returns an error when a section names no task, or two sections name the
/// same one.
pub fn agent_task_sections(markdown: &str) -> Result<BTreeMap<&str, Range<usize>>, String> {
    // Every heading of level one or two outside code: where it starts, and
    // the task it names, if it is a task's.
    let mut headings: Vec<(usize, Option<&str>)> = Vec::new();
    let mut fence: Option<&str> = None;
    let mut offset = 0;
    for raw in markdown.split_inclusive('\n') {
        let line = raw.trim_end_matches(['\n', '\r']);
        let marker = ["```", "~~~"]
            .into_iter()
            .find(|marker| line.starts_with(marker));
        match (marker, fence) {
            (Some(marker), None) => fence = Some(marker),
            (Some(marker), Some(fenced)) if marker == fenced => fence = None,
            (_, None) if is_section_heading(line) => {
                headings.push((offset, line.strip_prefix(AGENT_TASK_HEADING).map(str::trim)))
            }
            _ => {}
        }
        offset += raw.len();
    }
    let ends = headings
        .iter()
        .skip(1)
        .map(|(start, _)| *start)
        .chain([markdown.len()]);
    let mut sections = BTreeMap::new();
    for ((start, name), end) in headings.iter().zip(ends) {
        let Some(name) = *name else {
            continue;
        };
        if name.is_empty() {
            return Err(format!(
                "a \"{}\" section in SKILL.md names no task",
                AGENT_TASK_HEADING.trim_end()
            ));
        }
        if sections.insert(name, *start..end).is_some() {
            return Err(format!(
                "SKILL.md has two \"{AGENT_TASK_HEADING}{name}\" sections; each task has one"
            ));
        }
    }
    Ok(sections)
}

/// The manual a turn performing `task` is given: all of it, with every other
/// task's section left out. So the turn reads the skill's shared procedure
/// and its own task, and never another task's instructions. None when the
/// manual has no section for `task`.
///
/// # Errors
///
/// Returns an error when the manual's sections are malformed, as
/// [`agent_task_sections`] says.
pub fn agent_task_manual(markdown: &str, task: &str) -> Result<Option<String>, String> {
    let sections = agent_task_sections(markdown)?;
    if !sections.contains_key(task) {
        return Ok(None);
    }
    let mut others = sections
        .into_iter()
        .filter(|(name, _)| *name != task)
        .map(|(_, span)| span)
        .collect::<Vec<_>>();
    others.sort_by_key(|span| span.start);
    let mut manual = String::with_capacity(markdown.len());
    let mut kept_from = 0;
    for span in others {
        manual.push_str(&markdown[kept_from..span.start]);
        kept_from = span.end;
    }
    manual.push_str(&markdown[kept_from..]);
    Ok(Some(manual))
}

/// A heading of level one or two, which ends a task's section.
fn is_section_heading(line: &str) -> bool {
    ["#", "##"].into_iter().any(|marks| {
        line.strip_prefix(marks)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with([' ', '\t']))
    })
}

fn parse_yaml_object(source: &str, object_error: &str) -> Result<JsonObject, ParseError> {
    crate::assert_yaml_supported_subset("skill_frontmatter", source)?;
    let parsed: JsonValue =
        serde_norway::from_str(source).map_err(|error| ParseError::InvalidYaml {
            field: "skill_frontmatter".to_owned(),
            message: error.to_string(),
        })?;
    match parsed {
        JsonValue::Object(object) => Ok(object),
        _ => Err(ParseError::InvalidDocument {
            field: "skill_frontmatter".to_owned(),
            message: object_error.to_owned(),
        }),
    }
}

fn capture_string(captures: &regex::Captures<'_>, index: usize) -> Result<String, ParseError> {
    captures
        .get(index)
        .map(|value| value.as_str().to_owned())
        .ok_or_else(|| ParseError::InvalidDocument {
            field: "skill".to_owned(),
            message: "Skill markdown must start with YAML frontmatter delimited by ---.".to_owned(),
        })
}

#[cfg(test)]
mod tests {
    use super::{agent_task_manual, agent_task_sections};

    const MANUAL: &str = "---\nname: review\n---\n# Review\n\nThe shared procedure.\n\n## Agent task: draft\n\nYou write drafts.\n\n### Voice\n\nPlain.\n\n```md\n## Agent task: not-a-task\n# not a heading\n```\n\n## Agent task: check\n\nYou check drafts.\n\n## Notes\n\nFor every task.\n";

    #[test]
    fn a_section_runs_to_the_next_heading_of_level_one_or_two() -> Result<(), String> {
        let sections = agent_task_sections(MANUAL)?;
        assert_eq!(
            sections.keys().copied().collect::<Vec<_>>(),
            ["check", "draft"]
        );
        assert_eq!(
            &MANUAL[sections["draft"].clone()],
            "## Agent task: draft\n\nYou write drafts.\n\n### Voice\n\nPlain.\n\n```md\n## Agent task: not-a-task\n# not a heading\n```\n\n"
        );
        assert_eq!(
            &MANUAL[sections["check"].clone()],
            "## Agent task: check\n\nYou check drafts.\n\n"
        );
        let last = "# Skill\r\n\r\n## Agent task: only\r\n\r\nDo it.\r\n";
        assert_eq!(
            &last[agent_task_sections(last)?["only"].clone()],
            "## Agent task: only\r\n\r\nDo it.\r\n"
        );
        Ok(())
    }

    #[test]
    fn a_turn_reads_the_shared_manual_and_only_its_own_task() -> Result<(), String> {
        assert_eq!(
            agent_task_manual(MANUAL, "check")?.as_deref(),
            Some(
                "---\nname: review\n---\n# Review\n\nThe shared procedure.\n\n## Agent task: check\n\nYou check drafts.\n\n## Notes\n\nFor every task.\n"
            )
        );
        let draft = agent_task_manual(MANUAL, "draft")?.unwrap_or_default();
        assert!(draft.contains("You write drafts.") && draft.contains("The shared procedure."));
        assert!(!draft.contains("You check drafts."), "{draft}");
        assert_eq!(agent_task_manual(MANUAL, "publish")?, None);
        Ok(())
    }

    #[test]
    fn a_task_has_one_section_and_every_section_names_its_task() {
        let twice =
            agent_task_sections("## Agent task: draft\n\nOne.\n\n## Agent task: draft\n\nTwo.\n");
        assert!(
            twice
                .as_ref()
                .is_err_and(|message| message.contains("two \"## Agent task: draft\" sections")),
            "{twice:?}"
        );
        let unnamed = agent_task_sections("## Agent task:   \n\nWho?\n");
        assert!(
            unnamed
                .as_ref()
                .is_err_and(|message| message.contains("names no task")),
            "{unnamed:?}"
        );
    }
}
