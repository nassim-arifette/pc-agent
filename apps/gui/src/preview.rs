//! A local, single-use consent ticket binds the reviewed message to its immutable inputs.
use pcagent_agent::{conversation_message, PastTurn, Sharing};
use pcagent_shared::{new_id, Snapshot};
use serde_json::Value;
use std::time::{Duration, Instant};

const MAX_AGE: Duration = Duration::from_secs(300);

pub struct PreparedTurn {
    pub id: String,
    pub question: String,
    pub conversation_id: Option<String>,
    pub past: Vec<PastTurn>,
    pub snapshot: Snapshot,
    pub sharing: Sharing,
    pub message: Value,
    pub storage_scope: Option<String>,
    created: Instant,
}

impl PreparedTurn {
    pub fn new(
        question: String,
        conversation_id: Option<String>,
        past: Vec<PastTurn>,
        snapshot: Snapshot,
        sharing: Sharing,
        storage_scope: Option<String>,
    ) -> Self {
        let message = conversation_message(&past, &question, &snapshot, sharing);
        Self {
            id: new_id("preview"),
            question,
            conversation_id,
            past,
            snapshot,
            sharing,
            message,
            storage_scope,
            created: Instant::now(),
        }
    }
}

pub struct Reviewed<'a> {
    pub id: &'a str,
    pub question: &'a str,
    pub conversation_id: Option<&'a str>,
    pub past: &'a [PastTurn],
    pub sharing: Sharing,
    pub storage_scope: Option<&'a str>,
}

/// Consume only a matching ticket. No provider request or fresh collection occurs here.
pub fn take(
    slot: &mut Option<PreparedTurn>,
    reviewed: Reviewed<'_>,
) -> Result<PreparedTurn, String> {
    let Some(pending) = slot.as_ref() else {
        return Err("Open the sharing preview before sending.".into());
    };
    if pending.created.elapsed() > MAX_AGE {
        return Err("The preview expired. Open it again for a fresh measurement.".into());
    }
    if pending.id != reviewed.id
        || pending.question != reviewed.question
        || pending.conversation_id.as_deref() != reviewed.conversation_id
        || pending.past != reviewed.past
        || pending.sharing != reviewed.sharing
        || reviewed
            .storage_scope
            .is_some_and(|s| pending.storage_scope.as_deref() != Some(s))
    {
        return Err(
            "The question, conversation or sharing choice changed. Open the preview again.".into(),
        );
    }
    Ok(slot.take().expect("matching ticket exists"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcagent_agent::{converse, AgentError, Model, Reply, SnapshotTools, ToolChoice};
    use pcagent_shared::{Evidence, Metric, Window};

    fn snapshot(value: u64) -> Snapshot {
        let mut s = Snapshot::empty("private-local-session");
        s.evidence.push(Evidence::measured(
            Metric::PhysicalAvailableBytes,
            "system",
            value,
            "fixture",
            Window::instant(),
        ));
        s
    }
    fn pending() -> PreparedTurn {
        PreparedTurn::new(
            "Et maintenant ?".into(),
            Some("conversation".into()),
            vec![PastTurn {
                question: "Avant ?".into(),
                answer: "Une ancienne réponse.".into(),
                ..PastTurn::default()
            }],
            snapshot(2_000_000_000),
            Sharing::Anonymized,
            Some("scope-1".into()),
        )
    }
    fn reviewed<'a>(id: &'a str, past: &'a [PastTurn]) -> Reviewed<'a> {
        Reviewed {
            id,
            question: "Et maintenant ?",
            conversation_id: Some("conversation"),
            past,
            sharing: Sharing::Anonymized,
            storage_scope: Some("scope-1"),
        }
    }

    #[test]
    fn the_reviewed_capture_and_history_are_the_actual_first_message() {
        struct Capture(Vec<Value>);
        impl Model for Capture {
            fn complete(
                &mut self,
                messages: &[Value],
                _: &Value,
                _: ToolChoice,
                _: Duration,
            ) -> Result<Reply, AgentError> {
                self.0 = messages.to_vec();
                Err(AgentError::Cancelled)
            }
        }
        let p = pending();
        let (id, past, expected) = (p.id.clone(), p.past.clone(), p.message.clone());
        let mut slot = Some(p);
        let p = take(&mut slot, reviewed(&id, &past)).unwrap();
        let mut model = Capture(Vec::new());
        let mut tools = SnapshotTools::new(|_, _| snapshot(9_000_000_000));
        converse(
            &mut model,
            &p.past,
            &p.question,
            &p.snapshot,
            &mut tools,
            |_| {},
            |_, _| false,
        );
        let sent: Value = serde_json::from_str(model.0[1]["content"].as_str().unwrap()).unwrap();
        assert_eq!(sent, expected);
        assert_eq!(sent["previous_turns"][0]["question"], "Avant ?");
        assert!(
            take(&mut slot, reviewed(&id, &past)).is_err(),
            "tickets cannot be reused"
        );
    }

    #[test]
    fn stale_or_mismatched_consent_never_returns_a_capture() {
        for changed in 0..7 {
            let mut p = pending();
            if changed == 6 {
                p.created = Instant::now() - MAX_AGE - Duration::from_secs(1);
            }
            let (id, past) = (p.id.clone(), p.past.clone());
            let mut input = reviewed(&id, &past);
            match changed {
                0 => input.id = "different",
                1 => input.question = "different",
                2 => input.conversation_id = None,
                3 => input.past = &[],
                4 => input.sharing = Sharing::Complete,
                5 => input.storage_scope = Some("scope-2"),
                _ => {}
            }
            assert!(take(&mut Some(p), input).is_err(), "case {changed}");
        }
    }
}
