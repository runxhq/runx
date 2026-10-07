//! A connector's message rules, compiled: how its messages address people and
//! how untrusted text is made inert before it is sent. A composer such as the
//! assistant reads these instead of knowing any provider.

use std::path::Path;

use regex::Regex;

use super::{ConnectorScope, LoadedConnector};

pub struct Messages {
    actor: Regex,
    mention: String,
    inert: Vec<(Regex, String)>,
    active: Vec<String>,
    readback_normalization: Option<String>,
}

impl Messages {
    /// The rules connector `id` declares, as the skill in `skill_dir` sees
    /// it: an official connector or one of the skill's plugin.
    pub fn for_skill(skill_dir: &Path, id: &str) -> Result<Self, String> {
        let connector = ConnectorScope::for_skill(skill_dir)
            .and_then(|scope| scope.connector(id))
            .map_err(|error| error.to_string())?;
        Self::of(&connector)
    }

    /// The rules `connector` declares.
    pub fn of(connector: &LoadedConnector) -> Result<Self, String> {
        let id = connector.manifest.id.as_str();
        let rules = connector
            .manifest
            .messages
            .as_ref()
            .ok_or_else(|| format!("connector {id} declares no message rules"))?;
        let invalid = |error: regex::Error| format!("connector {id}'s message rules: {error}");
        Ok(Self {
            actor: runx_parser::whole_match(rules.actor.as_str()).map_err(invalid)?,
            mention: rules.mention.as_str().to_owned(),
            inert: rules
                .inert
                .iter()
                .map(|rule| {
                    Regex::new(rule.pattern.as_str())
                        .map(|pattern| (pattern, rule.replace.clone()))
                        .map_err(invalid)
                })
                .collect::<Result<_, _>>()?,
            active: rules
                .active
                .iter()
                .map(|token| token.as_str().to_owned())
                .collect(),
            readback_normalization: rules
                .readback_normalization
                .as_ref()
                .map(|normalization| normalization.as_str().to_owned()),
        })
    }

    /// How a message mentions `actor`, when it is an account the provider
    /// addresses.
    #[must_use]
    pub fn mention(&self, actor: &str) -> Option<String> {
        self.actor
            .is_match(actor)
            .then(|| self.mention.replace("{actor}", actor))
    }

    /// `text` with every rule applied in order.
    #[must_use]
    pub fn inert(&self, text: &str) -> String {
        self.inert
            .iter()
            .fold(text.to_owned(), |text, (pattern, replace)| {
                pattern.replace_all(&text, replace.as_str()).into_owned()
            })
    }

    /// Whether `text` addresses and links no one.
    #[must_use]
    pub fn is_inert(&self, text: &str) -> bool {
        !self
            .active
            .iter()
            .any(|token| text.contains(token.as_str()))
    }

    /// What a reconciling read reports it compared, if the provider shows a
    /// bounded preview.
    #[must_use]
    pub fn readback_normalization(&self) -> Option<&str> {
        self.readback_normalization.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::Messages;
    use crate::kernel::connectors::ConnectorScope;

    fn official(id: &str) -> Result<Messages, String> {
        let connector = ConnectorScope::official_only()
            .and_then(|scope| scope.connector(id))
            .map_err(|error| error.to_string())?;
        Messages::of(&connector)
    }

    #[test]
    fn slack_rules_mention_its_accounts_and_make_markup_inert() -> Result<(), String> {
        let slack = official("slack")?;
        assert_eq!(slack.mention("U123ABC").as_deref(), Some("<@U123ABC>"));
        assert_eq!(slack.mention("kam"), None);
        let text = slack.inert(
            "<@U123|Kam> @channel <!here> <https://example.com/a|the link> a & b `x` <unclosed",
        );
        assert_eq!(text, "Kam ＠channel group the link a  and  b 'x' (unclosed");
        assert!(slack.is_inert(&text));
        assert!(!slack.is_inert("hi @here"));
        assert_eq!(
            slack.readback_normalization(),
            Some("slack_bounded_preview")
        );
        assert!(official("github").is_err());
        Ok(())
    }
}
