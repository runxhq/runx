//! A failure is said once, by what decided it (rule 4). Whatever refuses or
//! fails a run, a step or a call makes one `Failure` where it decides; each
//! layer it passes through adds only its own address, and every surface
//! renders it with the one `Display` here, so a person or a model reads one
//! sentence: why, who decided, where, and what to do next.

use std::fmt;

use serde::{Deserialize, Serialize};

/// One refusal or failure, as the layer that decided it says it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Failure {
    /// What decided it, by the name its reader knows: a tool, an operation
    /// or a skill by its real name, a person, or runx's own layer (the run's
    /// authority, its cost limit, the worker, runx).
    pub decided_by: String,
    /// Where, as the run's record addresses it, from the run down: a step,
    /// a call's index after its step, and a child run's steps after the step
    /// or call that ran it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub at: Vec<String>,
    /// The deciding layer's own sentence.
    pub reason: String,
    /// What the person or agent can do about it, when there is something.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
}

impl Failure {
    /// The failure `decided_by` decided, for `reason`.
    pub fn new(decided_by: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            decided_by: decided_by.into(),
            at: Vec::new(),
            reason: reason.into(),
            next: None,
        }
    }

    /// The failure, saying what to do about it.
    #[must_use]
    pub fn next(mut self, next: impl Into<String>) -> Self {
        self.next = Some(next.into());
        self
    }

    /// The failure as the layer above sees it: `address` is where in that
    /// layer it happened, the step or the call that ran what failed.
    #[must_use]
    pub fn within(mut self, address: &str) -> Self {
        let mut at = address
            .split('/')
            .filter(|segment| !segment.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        at.append(&mut self.at);
        self.at = at;
        self
    }

    /// The failure placed at `step` of its run, once: one already placed at
    /// the step, or below it, stays where it is. A step's address may be
    /// more than one segment, as a call's step is.
    #[must_use]
    pub fn at_step(self, step: &str) -> Self {
        let segments = step.split('/').filter(|segment| !segment.is_empty());
        let mut placed = self.at.iter();
        if segments
            .clone()
            .all(|segment| placed.next().is_some_and(|at| at == segment))
        {
            return self;
        }
        self.within(step)
    }

    /// The step of its run it happened at, once placed.
    #[must_use]
    pub fn step(&self) -> Option<&str> {
        self.at.first().map(String::as_str)
    }
}

impl fmt::Display for Failure {
    /// `{reason} ({decided_by}, at {at}). Next: {next}`
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let reason = self.reason.trim().trim_end_matches('.');
        write!(formatter, "{reason} ({}", self.decided_by)?;
        if !self.at.is_empty() {
            write!(formatter, ", at {}", self.at.join("/"))?;
        }
        formatter.write_str(").")?;
        if let Some(next) = self
            .next
            .as_deref()
            .map(str::trim)
            .filter(|next| !next.is_empty())
        {
            let next = next.trim_end_matches('.');
            write!(formatter, " Next: {next}.")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::Failure;

    #[test]
    fn a_failure_reads_as_one_sentence() {
        let failure = Failure::new("call.read", "call answer/9 is not a call of run r.")
            .next("pass call as <step>/<index>, as run.read's events name it")
            .at_step("read")
            .at_step("read")
            .within("save/0");
        assert_eq!(
            failure.to_string(),
            "call answer/9 is not a call of run r (call.read, at save/0/read). Next: pass call as <step>/<index>, as run.read's events name it."
        );
    }

    #[test]
    fn a_child_step_named_as_its_parent_keeps_both_levels() {
        let failure = Failure::new("runx", "it failed")
            .at_step("draft")
            .within("draft");
        assert_eq!(failure.at, ["draft", "draft"]);
        assert_eq!(failure.to_string(), "it failed (runx, at draft/draft).");
    }
}
