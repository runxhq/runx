//! Shared model-facing tool definitions and reversible wire-name admission.

use std::collections::BTreeMap;

use runx_contracts::JsonValue;

#[derive(Clone, Debug)]
pub struct AgentToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: JsonValue,
}

#[derive(Clone, Debug, Default)]
pub struct AgentToolNameMap {
    definitions: Vec<AgentToolDefinition>,
    real_by_wire: BTreeMap<String, String>,
}

impl AgentToolNameMap {
    pub fn new(definitions: Vec<AgentToolDefinition>) -> Result<Self, &'static str> {
        let mut real_by_wire = BTreeMap::new();
        for definition in &definitions {
            let wire = wire_tool_name(&definition.name);
            if wire.is_empty()
                || wire.len() > 128
                || !wire
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
            {
                return Err("managed agent tool name is not valid on the model wire");
            }
            if real_by_wire.insert(wire, definition.name.clone()).is_some() {
                return Err("managed agent tool names collide on the model wire");
            }
        }
        Ok(Self {
            definitions,
            real_by_wire,
        })
    }

    pub fn definitions(&self) -> &[AgentToolDefinition] {
        &self.definitions
    }

    pub fn real_name(&self, wire: &str) -> Option<&str> {
        self.real_by_wire.get(wire).map(String::as_str)
    }
}

pub fn wire_tool_name(name: &str) -> String {
    name.replace('.', "_")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn definition(name: &str) -> AgentToolDefinition {
        AgentToolDefinition {
            name: name.to_owned(),
            description: String::new(),
            input_schema: JsonValue::Object(Default::default()),
        }
    }

    #[test]
    fn mapping_rejects_ambiguous_or_invalid_names() -> Result<(), String> {
        assert!(
            AgentToolNameMap::new(vec![definition("acme.post"), definition("acme_post")]).is_err()
        );
        assert!(AgentToolNameMap::new(vec![definition("bad/name")]).is_err());
        let map = AgentToolNameMap::new(vec![definition("acme.post")])?;
        assert_eq!(map.real_name("acme_post"), Some("acme.post"));
        assert_eq!(map.real_name("unoffered"), None);
        Ok(())
    }
}
