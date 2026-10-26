//! One bounded, replaceable draft. Only its opaque reference crosses the send boundary.
use crate::failure::{Code, Failure};
use pcagent_agent::image_input::{ImageError, ImageInfo, ImageInput};
use pcagent_shared::new_id;
use serde::Serialize;
use std::time::{Duration, Instant};

const MAX_AGE: Duration = Duration::from_secs(300);

#[derive(Clone)]
pub struct PreparedImage {
    pub id: String,
    pub image: ImageInput,
    conversation_id: Option<String>,
    expires_at: Instant,
}

#[derive(Serialize)]
pub struct ImageView {
    pub id: String,
    #[serde(flatten)]
    pub info: ImageInfo,
    pub data_url: String,
}

impl PreparedImage {
    pub fn view(&self) -> ImageView {
        ImageView::new(&self.id, &self.image)
    }

    pub fn expired(&self) -> bool {
        self.expires_at <= Instant::now()
    }
}

impl ImageView {
    pub fn new(id: &str, image: &ImageInput) -> Self {
        Self {
            id: id.into(),
            info: image.info().clone(),
            data_url: image.data_url(),
        }
    }
}

#[derive(Default)]
pub struct ImageSlot {
    revision: u64,
    draft: Option<PreparedImage>,
}

impl ImageSlot {
    pub fn begin(&mut self) -> u64 {
        self.revision = self.revision.wrapping_add(1);
        self.draft = None;
        self.revision
    }

    pub fn finish(&mut self, revision: u64, image: ImageInput) -> Result<ImageView, Failure> {
        if revision != self.revision {
            return Err(changed());
        }
        let draft = PreparedImage {
            id: new_id("image"),
            image,
            conversation_id: None,
            expires_at: Instant::now() + MAX_AGE,
        };
        let view = draft.view();
        self.draft = Some(draft);
        Ok(view)
    }

    pub fn bind(
        &mut self,
        id: &str,
        conversation_id: Option<&str>,
    ) -> Result<PreparedImage, Failure> {
        let draft = self
            .draft
            .as_mut()
            .filter(|draft| draft.id == id && !draft.expired())
            .ok_or_else(expired)?;
        draft.conversation_id = conversation_id.map(str::to_owned);
        Ok(draft.clone())
    }

    pub fn matches(&self, image: &PreparedImage) -> bool {
        self.draft.as_ref().is_some_and(|draft| {
            !draft.expired()
                && !image.expired()
                && draft.id == image.id
                && draft.image.info() == image.image.info()
        })
    }

    pub fn discard(&mut self, id: &str) -> bool {
        if self.draft.as_ref().is_some_and(|draft| draft.id == id) {
            self.begin();
            true
        } else {
            false
        }
    }

    pub fn invalidate_conversation(&mut self, id: &str) {
        if self
            .draft
            .as_ref()
            .is_some_and(|draft| draft.conversation_id.as_deref() == Some(id))
        {
            self.begin();
        }
    }
}

pub fn expired() -> Failure {
    Failure::new(
        Code::ImageExpired,
        "Attach the image again; its local draft expired or was removed.",
    )
}

pub fn changed() -> Failure {
    Failure::new(
        Code::PreviewChanged,
        "The image changed. Open the preview again.",
    )
}

pub fn preparation_error(error: ImageError) -> Failure {
    let code = match error {
        ImageError::TooLarge => Code::ImageTooLarge,
        ImageError::Invalid => Code::ImageInvalid,
        ImageError::Unsupported | ImageError::Animated => Code::ImageUnsupported,
    };
    Failure::new(code, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcagent_agent::image_input::ImageRecord;

    fn image() -> ImageInput {
        ImageInput::from_record(ImageRecord::new(vec![0xff, 0xd8, 0xff, 0xd9], 1, 1).unwrap())
    }

    #[test]
    fn replacement_revokes_old_ids_and_late_preparation_cannot_win() {
        let mut slot = ImageSlot::default();
        let first = slot.begin();
        let next = slot.begin();
        assert!(slot.finish(first, image()).is_err());
        let current = slot.finish(next, image()).unwrap();
        let frozen = slot.bind(&current.id, Some("conversation")).unwrap();
        assert!(slot.matches(&frozen));
        let next = slot.begin();
        let replacement = slot.finish(next, image()).unwrap();
        assert!(!slot.matches(&frozen));
        assert!(!slot.discard(&current.id));
        assert!(slot.bind(&replacement.id, None).is_ok());
    }

    #[test]
    fn deletion_and_expiry_revoke_a_frozen_attachment() {
        let mut slot = ImageSlot::default();
        let next = slot.begin();
        let view = slot.finish(next, image()).unwrap();
        let frozen = slot.bind(&view.id, Some("conversation")).unwrap();
        slot.invalidate_conversation("other");
        assert!(slot.matches(&frozen));
        slot.invalidate_conversation("conversation");
        assert!(!slot.matches(&frozen));
        let next = slot.begin();
        let view = slot.finish(next, image()).unwrap();
        slot.draft.as_mut().unwrap().expires_at = Instant::now();
        assert!(slot.bind(&view.id, None).is_err());
    }
}
