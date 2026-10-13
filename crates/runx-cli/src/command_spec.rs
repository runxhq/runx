use serde::Serialize;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandSpec {
    pub name: &'static str,
    pub top_level_usage: &'static [&'static str],
    pub usage: &'static [&'static str],
    pub notes: &'static [&'static str],
    pub options: &'static [&'static str],
}

mod catalog;

pub use self::catalog::{COMMAND_SPECS, ROOT_COMMAND_SPEC};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CommandCatalog {
    schema: &'static str,
    root: &'static CommandSpec,
    commands: &'static [CommandSpec],
}

pub fn command_spec(name: &str) -> Option<&'static CommandSpec> {
    COMMAND_SPECS.iter().find(|spec| spec.name == name)
}

pub fn catalog_json() -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(&CommandCatalog {
        schema: "runx.cli_command_catalog.v1",
        root: &ROOT_COMMAND_SPEC,
        commands: COMMAND_SPECS,
    })
}

pub fn help_text() -> String {
    let mut output = String::from("runx\n\nUsage:\n");
    for usage in ROOT_COMMAND_SPEC.usage {
        output.push_str("  ");
        output.push_str(usage);
        output.push('\n');
    }
    output.push_str("\nCommands:\n");
    for spec in COMMAND_SPECS {
        let usage_lines = if spec.top_level_usage.is_empty() {
            spec.usage
        } else {
            spec.top_level_usage
        };
        for usage in usage_lines {
            output.push_str("  ");
            output.push_str(usage);
            output.push('\n');
        }
    }
    output
}

pub fn command_help_text(name: &str) -> Option<String> {
    let spec = command_spec(name)?;
    let mut output = format!("runx {}\n\nUsage:\n", spec.name);
    for usage in spec.usage {
        output.push_str("  ");
        output.push_str(usage);
        output.push('\n');
    }
    if !spec.notes.is_empty() {
        output.push('\n');
        for note in spec.notes {
            output.push_str(note);
            output.push('\n');
        }
    }
    if !spec.options.is_empty() {
        output.push_str("\nOptions:\n");
        for option in spec.options {
            output.push_str("  ");
            output.push_str(option);
            output.push('\n');
        }
    }
    Some(output)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{COMMAND_SPECS, ROOT_COMMAND_SPEC, catalog_json, command_help_text, help_text};

    #[test]
    fn command_names_are_unique_and_have_help() {
        let mut names = BTreeSet::new();
        for spec in COMMAND_SPECS {
            assert!(names.insert(spec.name), "duplicate command {}", spec.name);
            let help = command_help_text(spec.name);
            assert!(help.is_some(), "missing help for {}", spec.name);
        }
        assert_eq!(COMMAND_SPECS.len(), 17);
    }

    #[test]
    fn top_level_help_is_generated_from_command_usage() {
        let help = help_text();
        for spec in COMMAND_SPECS {
            let usage_lines = if spec.top_level_usage.is_empty() {
                spec.usage
            } else {
                spec.top_level_usage
            };
            for usage in usage_lines {
                assert!(help.lines().any(|line| line.trim() == *usage));
            }
        }
    }

    #[test]
    fn json_catalog_is_a_direct_projection_of_native_help_specs()
    -> Result<(), Box<dyn std::error::Error>> {
        let catalog: serde_json::Value = serde_json::from_str(&catalog_json()?)?;

        assert_eq!(catalog["schema"], "runx.cli_command_catalog.v1");
        assert_eq!(catalog["root"]["name"], ROOT_COMMAND_SPEC.name);
        assert_eq!(
            catalog["commands"].as_array().map(Vec::len),
            Some(COMMAND_SPECS.len())
        );
        let connector = catalog["commands"]
            .as_array()
            .and_then(|commands| {
                commands
                    .iter()
                    .find(|command| command["name"] == "connector")
            })
            .ok_or_else(|| std::io::Error::other("connector command missing from catalog"))?;
        assert!(
            connector["options"]
                .as_array()
                .is_some_and(|options| options.iter().any(|option| {
                    option
                        .as_str()
                        .is_some_and(|option| option.starts_with("--credential "))
                }))
        );
        Ok(())
    }
}
