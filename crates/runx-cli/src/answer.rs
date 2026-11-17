//! A person answers a paused run at their terminal (the loop contract's §9;
//! ledger row 33). Each request the run waits on is shown whole and answered
//! in place, and the answers become the same `runx.resume.v1` document a file
//! would hold, so resuming goes on exactly as it does from a file.

use std::io::{self, BufRead, Write};

use runx_contracts::schema::NonEmptyString;
use runx_contracts::{
    ApprovalGate, BoundApproval, JsonObject, JsonValue, PurchaseRequest, Question,
    ResolutionRequest, ResumeDocument, WaitedRequest,
};

/// Ask the person about each of `waiting` on `output`, reading their answers
/// from `input`. A request no person answers here (an agent's turn, a retry or
/// a wait runx settles itself) is said and left. An empty answer to a yes or
/// no question is no.
pub(crate) fn answer_at_terminal(
    waiting: &[WaitedRequest],
    input: &mut dyn BufRead,
    output: &mut dyn Write,
) -> Result<ResumeDocument, String> {
    let mut document = ResumeDocument {
        answers: Default::default(),
        approvals: Default::default(),
        standing_approvals: None,
        close: None,
    };
    let mut terminal = Terminal { input, output };
    for (place, waited) in waiting.iter().enumerate() {
        terminal.say(&format!(
            "\n{} of {}: {}",
            place + 1,
            waiting.len(),
            waited.address
        ))?;
        let digest = &waited.request_digest;
        let answer = match &waited.request {
            ResolutionRequest::Approval { gate, .. } => Some(terminal.approve(gate, digest)?),
            ResolutionRequest::Input { questions, .. } => {
                Some(terminal.questions(questions, digest)?)
            }
            ResolutionRequest::Purchase { purchase, .. } => {
                Some(terminal.purchase(purchase, digest)?)
            }
            ResolutionRequest::Turn { .. } => {
                terminal.say(
                    "  An agent's turn: drive it with --drive, or give it to an agent with runx mcp --run.",
                )?;
                None
            }
            ResolutionRequest::Retry { reason, .. } => {
                terminal.say(&format!("  runx tries this again itself: {reason}"))?;
                None
            }
            ResolutionRequest::Wait { until, .. } => {
                terminal.say(&format!("  It waits until {until}, and goes on by itself."))?;
                None
            }
        };
        if let Some(answer) = answer {
            document
                .approvals
                .insert(waited.address.as_str().to_owned(), answer);
        }
    }
    Ok(document)
}

/// What the person sees and types.
struct Terminal<'a> {
    input: &'a mut dyn BufRead,
    output: &'a mut dyn Write,
}

/// An answer bound to the request it answers, by its digest.
fn bound(request_digest: &NonEmptyString) -> BoundApproval {
    BoundApproval {
        request_digest: request_digest.clone(),
        approved: None,
        reason: None,
        choice: None,
        value: None,
    }
}

impl Terminal<'_> {
    fn say(&mut self, line: &str) -> Result<(), String> {
        writeln!(self.output, "{line}").map_err(|error| error.to_string())
    }

    /// One line the person types, trimmed. The input ending is refused, so a
    /// closed terminal never reads as an answer.
    fn ask(&mut self, prompt: &str) -> Result<String, String> {
        write!(self.output, "{prompt}")
            .and_then(|()| self.output.flush())
            .map_err(|error| error.to_string())?;
        let mut line = String::new();
        let read = self
            .input
            .read_line(&mut line)
            .map_err(|error| error.to_string())?;
        if read == 0 {
            return Err("the terminal closed before every request was answered; nothing was resumed".to_owned());
        }
        Ok(line.trim().to_owned())
    }

    fn yes(&mut self, prompt: &str) -> Result<bool, String> {
        Ok(matches!(
            self.ask(prompt)?.to_ascii_lowercase().as_str(),
            "y" | "yes"
        ))
    }

    /// The person's words why, when they give some.
    fn why(&mut self) -> Result<Option<NonEmptyString>, String> {
        Ok(NonEmptyString::new(self.ask("  Why? (enter to skip) ")?))
    }

    fn json(&mut self, value: &JsonValue) -> Result<(), String> {
        let text = serde_json::to_string_pretty(value).map_err(|error| error.to_string())?;
        for line in text.lines() {
            self.say(&format!("    {line}"))?;
        }
        Ok(())
    }

    /// The exact act, as the gate holds it, and the person's yes or no.
    fn approve(
        &mut self,
        gate: &ApprovalGate,
        digest: &NonEmptyString,
    ) -> Result<BoundApproval, String> {
        self.say(&format!("  {}", gate.reason))?;
        if let Some(summary) = &gate.summary {
            self.json(&JsonValue::Object(summary.clone()))?;
        }
        if let Some(payload) = &gate.payload {
            self.say("  Exactly:")?;
            self.json(payload)?;
        }
        if let Some(artifact) = &gate.payload_artifact {
            self.say(&format!(
                "  Exactly what {} holds",
                serde_json::to_string(artifact).map_err(|error| error.to_string())?
            ))?;
        }
        let approved = self.yes("  Approve? [y/N] ")?;
        Ok(BoundApproval {
            approved: Some(approved),
            reason: if approved { None } else { self.why()? },
            ..bound(digest)
        })
    }

    /// Each question in turn: an option by its number, or the person's own
    /// words where they may give them. A required question is asked again
    /// until it's answered.
    fn questions(
        &mut self,
        questions: &[Question],
        digest: &NonEmptyString,
    ) -> Result<BoundApproval, String> {
        let mut answers = JsonObject::new();
        for question in questions {
            self.say(&format!("  {}", question.prompt))?;
            if let Some(description) = &question.description {
                self.say(&format!("  {description}"))?;
            }
            for (number, option) in question.options.iter().enumerate() {
                let detail = option
                    .detail
                    .as_deref()
                    .map(|detail| format!(": {detail}"))
                    .unwrap_or_default();
                self.say(&format!("    {}. {}{detail}", number + 1, option.label))?;
            }
            let answer = loop {
                let typed = self.ask("  > ")?;
                let chosen = typed
                    .parse::<usize>()
                    .ok()
                    .and_then(|number| number.checked_sub(1))
                    .and_then(|index| question.options.get(index))
                    .map(|option| option.label.as_str().to_owned());
                match chosen {
                    Some(label) => break Some(label),
                    None if typed.is_empty() && !question.required => break None,
                    None if !typed.is_empty()
                        && (question.free_text || question.options.is_empty()) =>
                    {
                        break Some(typed);
                    }
                    None => self.say(if question.options.is_empty() {
                        "  This one needs an answer."
                    } else {
                        "  Choose one by its number."
                    })?,
                }
            };
            if let Some(answer) = answer {
                answers.insert(question.id.as_str().to_owned(), JsonValue::String(answer));
            }
        }
        Ok(BoundApproval {
            value: Some(JsonValue::Object(answers)),
            ..bound(digest)
        })
    }

    /// What a purchase buys, from whom and for how much, then the person's yes
    /// and the method they pay with.
    fn purchase(
        &mut self,
        purchase: &PurchaseRequest,
        digest: &NonEmptyString,
    ) -> Result<BoundApproval, String> {
        let shown = serde_json::to_value(purchase)
            .and_then(serde_json::from_value::<JsonValue>)
            .map_err(|error| error.to_string())?;
        let mut text = Vec::new();
        if let JsonValue::Object(object) = &shown {
            crate::skill::output::write_purchase(&mut text, object)
                .map_err(|error| error.to_string())?;
        }
        self.say(String::from_utf8_lossy(&text).trim_end())?;
        if !self.yes("  Buy it? [y/N] ")? {
            return Ok(BoundApproval {
                approved: Some(false),
                reason: self.why()?,
                ..bound(digest)
            });
        }
        let choices = purchase
            .methods
            .iter()
            .map(|method| method.choice().to_owned())
            .collect::<Vec<_>>();
        let choice = match choices.as_slice() {
            [only] => only.clone(),
            _ => loop {
                for (number, choice) in choices.iter().enumerate() {
                    self.say(&format!("    {}. {choice}", number + 1))?;
                }
                let typed = self.ask("  Pay with > ")?;
                if let Some(choice) = typed
                    .parse::<usize>()
                    .ok()
                    .and_then(|number| number.checked_sub(1))
                    .and_then(|index| choices.get(index))
                {
                    break choice.clone();
                }
            },
        };
        Ok(BoundApproval {
            approved: Some(true),
            choice: NonEmptyString::new(choice),
            ..bound(digest)
        })
    }
}

/// The person's terminal: standard input and output.
pub(crate) fn answer_at_this_terminal(waiting: &[WaitedRequest]) -> Result<ResumeDocument, String> {
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let mut output = io::stdout();
    answer_at_terminal(waiting, &mut input, &mut output)
}

#[cfg(test)]
mod tests {
    use super::answer_at_terminal;
    use runx_contracts::{ResolutionRequest, WaitedRequest};

    fn waited(value: serde_json::Value) -> Result<WaitedRequest, serde_json::Error> {
        serde_json::from_value(value)
    }

    #[test]
    fn a_person_approves_one_act_and_declines_another_with_their_words()
    -> Result<(), Box<dyn std::error::Error>> {
        let gate = |id: &str, text: &str| {
            serde_json::json!({
                "address": id,
                "request_digest": format!("sha256:{id}"),
                "request": {"kind": "approval", "id": id, "gate": {
                    "id": id, "reason": "Send one email", "payload": {"to": "priya@northwind.example", "body": text}
                }},
            })
        };
        let waiting = vec![waited(gate("first", "Hello"))?, waited(gate("second", "Hi"))?];
        let mut shown = Vec::new();
        let document = answer_at_terminal(
            &waiting,
            &mut "y\nn\nToo pushy for a first email\n".as_bytes(),
            &mut shown,
        )?;
        let shown = String::from_utf8(shown)?;
        assert!(shown.contains("priya@northwind.example") && shown.contains("Hello"));
        let first = &document.approvals["first"];
        assert_eq!(first.request_digest.as_str(), "sha256:first");
        assert_eq!((first.approved, first.reason.as_ref()), (Some(true), None));
        let second = &document.approvals["second"];
        assert_eq!(second.approved, Some(false));
        assert_eq!(
            second.reason.as_ref().map(|reason| reason.as_str()),
            Some("Too pushy for a first email")
        );
        Ok(())
    }

    #[test]
    fn a_question_is_answered_by_its_option_or_in_the_persons_words()
    -> Result<(), Box<dyn std::error::Error>> {
        let waiting = vec![waited(serde_json::json!({
            "address": "brief",
            "request_digest": "sha256:brief",
            "request": {"kind": "input", "id": "brief", "questions": [
                {"id": "tone", "prompt": "Which tone?", "required": true, "type": "choice",
                 "options": [{"label": "Plain"}, {"label": "Warm"}], "free_text": false},
                {"id": "note", "prompt": "Anything else?", "required": true, "type": "text",
                 "options": [], "free_text": true}
            ]},
        }))?];
        let document = answer_at_terminal(
            &waiting,
            &mut "7\n2\n\nKeep it short\n".as_bytes(),
            &mut Vec::new(),
        )?;
        assert_eq!(
            document.approvals["brief"].value,
            Some(serde_json::from_value(serde_json::json!({"tone": "Warm", "note": "Keep it short"}))?)
        );
        Ok(())
    }

    #[test]
    fn a_closed_terminal_is_no_answer_and_a_wait_asks_nothing()
    -> Result<(), Box<dyn std::error::Error>> {
        let waiting = vec![waited(serde_json::json!({
            "address": "send",
            "request_digest": "sha256:send",
            "request": {"kind": "approval", "id": "send", "gate": {"id": "send", "reason": "Send"}},
        }))?];
        assert!(answer_at_terminal(&waiting, &mut "".as_bytes(), &mut Vec::new()).is_err());
        let wait = vec![waited(serde_json::json!({
            "address": "later",
            "request_digest": "sha256:later",
            "request": {"kind": "wait", "id": "later", "until": 1_700_000_000_000_i64},
        }))?];
        assert!(matches!(wait[0].request, ResolutionRequest::Wait { .. }));
        let document = answer_at_terminal(&wait, &mut "".as_bytes(), &mut Vec::new())?;
        assert!(document.approvals.is_empty());
        Ok(())
    }
}
