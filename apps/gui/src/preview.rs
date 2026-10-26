//! A local, single-use consent ticket binds the reviewed message to its immutable inputs.
use crate::failure::{Code, Failure};
use pcagent_agent::{
    conversation_message_with_privacy, PastTurn, PrivacyContext, Sharing, StorageContext,
};
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
    pub privacy: PrivacyContext,
    pub storage_context: StorageContext,
    pub file_names: bool,
    pub routing: Option<crate::model_routing::RoutingReview>,
    pub image: Option<crate::images::PreparedImage>,
    expires_at: Instant,
}

impl PreparedTurn {
    /// Protect a credential supplied by the host before showing the first-message preview.
    pub fn protect_secret(&mut self, secret: &str) {
        self.privacy.protect_secret(secret);
        self.message = conversation_message_with_privacy(
            &self.past,
            &self.question,
            &self.snapshot,
            self.sharing,
            &self.privacy,
        );
    }

    pub fn new(
        question: String,
        conversation_id: Option<String>,
        past: Vec<PastTurn>,
        snapshot: Snapshot,
        sharing: Sharing,
        storage_scope: Option<String>,
    ) -> Self {
        let privacy = PrivacyContext::local();
        let message =
            conversation_message_with_privacy(&past, &question, &snapshot, sharing, &privacy);
        Self {
            id: new_id("preview"),
            question,
            conversation_id,
            past,
            snapshot,
            sharing,
            message,
            storage_scope,
            privacy,
            storage_context: StorageContext::default(),
            file_names: false,
            routing: None,
            image: None,
            expires_at: Instant::now() + MAX_AGE,
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
    pub file_names: bool,
    pub image_id: Option<&'a str>,
}

/// Consume only a matching ticket. No provider request or fresh collection occurs here.
pub fn take(
    slot: &mut Option<PreparedTurn>,
    reviewed: Reviewed<'_>,
) -> Result<PreparedTurn, Failure> {
    let Some(pending) = slot.as_ref() else {
        return Err(Failure::new(
            Code::PreviewRequired,
            "Open the sharing preview before sending.",
        ));
    };
    if pending.expires_at <= Instant::now() {
        return Err(Failure::new(
            Code::PreviewExpired,
            "The preview expired. Open it again for a fresh measurement.",
        ));
    }
    if pending.image.as_ref().is_some_and(|image| image.expired()) {
        return Err(crate::images::expired());
    }
    if pending.id != reviewed.id
        || pending.question != reviewed.question
        || pending.conversation_id.as_deref() != reviewed.conversation_id
        || pending.past != reviewed.past
        || pending.sharing != reviewed.sharing
        || pending.file_names != reviewed.file_names
        || pending.image.as_ref().map(|image| image.id.as_str()) != reviewed.image_id
        || reviewed
            .storage_scope
            .is_some_and(|s| pending.storage_scope.as_deref() != Some(s))
    {
        return Err(Failure::new(
            Code::PreviewChanged,
            "The question, conversation or sharing choice changed. Open the preview again.",
        ));
    }
    Ok(slot.take().expect("matching ticket exists"))
}

/// Revoke a deleted conversation's frozen context before its deletion command returns.
pub fn invalidate_conversation(slot: &mut Option<PreparedTurn>, conversation_id: &str) {
    if slot
        .as_ref()
        .is_some_and(|pending| pending.conversation_id.as_deref() == Some(conversation_id))
    {
        *slot = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcagent_agent::{
        converse_with_privacy, AgentError, Model, Reply, SnapshotTools, ToolChoice,
    };
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
            file_names: false,
            image_id: None,
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
        converse_with_privacy(
            &mut model,
            &p.past,
            &p.question,
            &p.snapshot,
            &mut tools,
            &p.privacy,
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
        for changed in 0..8 {
            let mut p = pending();
            if changed == 6 {
                p.expires_at = Instant::now();
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
                7 => input.file_names = true,
                _ => {}
            }
            let refused = take(&mut Some(p), input).err().expect("refused");
            // The page translates why nothing was sent.
            let expected = if changed == 6 {
                Code::PreviewExpired
            } else {
                Code::PreviewChanged
            };
            assert_eq!(refused.code, expected, "case {changed}");
        }
        let (id, past) = (String::from("preview-none"), Vec::new());
        let missing = take(&mut None, reviewed(&id, &past))
            .err()
            .expect("refused");
        assert_eq!(missing.code, Code::PreviewRequired);
    }

    #[test]
    fn deleting_a_conversation_revokes_its_preview_even_after_the_past_was_read() {
        let p = pending();
        let (id, previously_read_past) = (p.id.clone(), p.past.clone());
        let mut slot = Some(p);
        invalidate_conversation(&mut slot, "unrelated-conversation");
        assert!(slot.is_some());
        invalidate_conversation(&mut slot, "conversation");
        assert!(take(&mut slot, reviewed(&id, &previously_read_past)).is_err());
        assert!(slot.is_none());
    }

    #[test]
    fn a_file_loaded_credential_is_filtered_before_the_preview_is_published() {
        let mut p = PreparedTurn::new(
            "Why is opaque-fixture-key present?".into(),
            None,
            Vec::new(),
            snapshot(1),
            Sharing::Complete,
            None,
        );
        p.protect_secret("opaque-fixture-key");
        assert!(!p.message.to_string().contains("opaque-fixture-key"));
        assert_eq!(
            p.message,
            conversation_message_with_privacy(
                &p.past,
                &p.question,
                &p.snapshot,
                p.sharing,
                &p.privacy
            )
        );
    }

    #[test]
    fn image_ticket_binds_exact_bytes_and_cannot_add_remove_or_replace_the_image() {
        use crate::images::ImageSlot;
        use pcagent_agent::image_input::{ImageInput, ImageRecord};

        let mut images = ImageSlot::default();
        let revision = images.begin();
        let image =
            ImageInput::from_record(ImageRecord::new(vec![0xff, 0xd8, 0xff, 0xd9], 1, 1).unwrap());
        let view = images.finish(revision, image).unwrap();
        let mut pending = pending();
        pending.sharing = Sharing::Complete;
        pending.image = Some(images.bind(&view.id, Some("conversation")).unwrap());
        let (id, past) = (pending.id.clone(), pending.past.clone());
        let mut slot = Some(pending);
        for image_id in [None, Some("a-different-image")] {
            let mut input = reviewed(&id, &past);
            input.sharing = Sharing::Complete;
            input.image_id = image_id;
            assert_eq!(
                take(&mut slot, input).err().unwrap().code,
                Code::PreviewChanged
            );
            assert!(slot.is_some());
        }
        let mut input = reviewed(&id, &past);
        input.sharing = Sharing::Complete;
        input.image_id = Some(&view.id);
        let approved = take(&mut slot, input).unwrap();
        assert_eq!(approved.image.unwrap().view().data_url, view.data_url);
        assert!(slot.is_none());
        let mut input = reviewed(&id, &past);
        input.image_id = Some(&view.id);
        assert_eq!(
            take(&mut slot, input).err().unwrap().code,
            Code::PreviewRequired
        );
    }
}
