//! A voluntarily attached image is untrusted context, never measured evidence.
use base64::{engine::general_purpose::STANDARD, Engine};
use image::{
    codecs::{jpeg::JpegEncoder, png::PngDecoder},
    imageops::FilterType,
    DynamicImage, ImageDecoder, ImageFormat, ImageReader, Limits, Rgb, RgbImage,
};
pub use pcagent_store::{ImageInfo, ImageRecord};
use serde_json::{json, Value};
use std::{fmt, io::Cursor};

pub const MODEL_ID: &str = "nvidia/Nemotron-3.5-Super-VL";
pub const MAX_INPUT_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_OUTPUT_BYTES: usize = pcagent_store::MAX_IMAGE_BYTES;
pub const MAX_SIDE: u32 = pcagent_store::MAX_IMAGE_SIDE;
const MAX_INPUT_SIDE: u32 = 16_384;
const MAX_INPUT_PIXELS: u64 = 24_000_000;
const MAX_DECODE_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ImageError {
    #[error("Choose a static PNG or JPEG image.")]
    Unsupported,
    #[error("The image could not be decoded.")]
    Invalid,
    #[error("The image exceeds the permitted size or dimensions.")]
    TooLarge,
    #[error("Animated images are not supported.")]
    Animated,
}

#[derive(Clone)]
pub struct ImageInput {
    record: ImageRecord,
}

impl fmt::Debug for ImageInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImageInput")
            .field("info", self.info())
            .finish()
    }
}

impl ImageInput {
    pub fn prepare_base64(encoded: &str) -> Result<Self, ImageError> {
        if encoded.len() > MAX_INPUT_BYTES.div_ceil(3) * 4 {
            return Err(ImageError::TooLarge);
        }
        let bytes = STANDARD.decode(encoded).map_err(|_| ImageError::Invalid)?;
        Self::prepare(&bytes)
    }

    pub fn prepare(bytes: &[u8]) -> Result<Self, ImageError> {
        if bytes.len() > MAX_INPUT_BYTES {
            return Err(ImageError::TooLarge);
        }
        let format = image::guess_format(bytes).map_err(|_| ImageError::Unsupported)?;
        if !matches!(format, ImageFormat::Png | ImageFormat::Jpeg) {
            return Err(ImageError::Unsupported);
        }
        let complete = match format {
            ImageFormat::Jpeg => bytes.ends_with(&[0xff, 0xd9]),
            ImageFormat::Png => bytes.ends_with(b"\0\0\0\0IEND\xae\x42\x60\x82"),
            _ => false,
        };
        if !complete {
            return Err(ImageError::Invalid);
        }
        let mut limits = Limits::default();
        limits.max_image_width = Some(MAX_INPUT_SIDE);
        limits.max_image_height = Some(MAX_INPUT_SIDE);
        limits.max_alloc = Some(MAX_DECODE_BYTES);
        let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
        reader.limits(limits.clone());
        let mut decoder = reader.into_decoder().map_err(decode_error)?;
        let (width, height) = decoder.dimensions();
        if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_INPUT_PIXELS {
            return Err(ImageError::TooLarge);
        }
        if format == ImageFormat::Png {
            let png = PngDecoder::with_limits(Cursor::new(bytes), limits).map_err(decode_error)?;
            if png.is_apng().map_err(decode_error)? {
                return Err(ImageError::Animated);
            }
        }
        let orientation = decoder.orientation().map_err(decode_error)?;
        let mut decoded = DynamicImage::from_decoder(decoder).map_err(decode_error)?;
        decoded.apply_orientation(orientation);
        let rgba = decoded.into_rgba8();
        let rgb = RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
            let pixel = rgba.get_pixel(x, y);
            let alpha = u32::from(pixel[3]);
            Rgb(std::array::from_fn(|channel| {
                ((u32::from(pixel[channel]) * alpha + 255 * (255 - alpha) + 127) / 255) as u8
            }))
        });
        let mut prepared = DynamicImage::ImageRgb8(rgb);
        if prepared.width() > MAX_SIDE || prepared.height() > MAX_SIDE {
            prepared = prepared.resize(MAX_SIDE, MAX_SIDE, FilterType::Triangle);
        }
        let mut jpeg = Vec::new();
        JpegEncoder::new_with_quality(&mut jpeg, 85)
            .encode_image(&prepared)
            .map_err(decode_error)?;
        if jpeg.len() > MAX_OUTPUT_BYTES {
            return Err(ImageError::TooLarge);
        }
        let record = ImageRecord::new(jpeg, prepared.width(), prepared.height())
            .map_err(|_| ImageError::Invalid)?;
        Ok(Self::from_record(record))
    }

    pub fn from_record(record: ImageRecord) -> Self {
        Self { record }
    }

    pub fn record(&self) -> &ImageRecord {
        &self.record
    }

    pub fn info(&self) -> &ImageInfo {
        self.record.info()
    }

    pub fn bytes(&self) -> &[u8] {
        self.record.bytes()
    }

    pub fn data_url(&self) -> String {
        format!("data:image/jpeg;base64,{}", STANDARD.encode(self.bytes()))
    }

    pub fn message_content(&self, text: &Value) -> Value {
        json!([
            {"type": "text", "text": text.to_string()},
            {"type": "image_url", "image_url": {"url": self.data_url()}}
        ])
    }
}

fn decode_error(error: image::ImageError) -> ImageError {
    match error {
        image::ImageError::Limits(_) => ImageError::TooLarge,
        _ => ImageError::Invalid,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        conversation_message_with_privacy, converse_with_image, converse_with_privacy, AgentError,
        Args, Model, PastTurn, PrivacyContext, Reply, Sharing, ToolChoice, ToolOutput, ToolSpec,
        Toolbox,
    };
    use image::{codecs::png::PngEncoder, ImageEncoder, Rgba, RgbaImage};
    use pcagent_shared::Snapshot;
    use sha2::{Digest, Sha256};
    use std::time::Duration;

    fn png(width: u32, height: u32, pixel: Rgba<u8>) -> Vec<u8> {
        let image = RgbaImage::from_pixel(width, height, pixel);
        let mut bytes = Vec::new();
        PngEncoder::new(&mut bytes)
            .write_image(
                image.as_raw(),
                width,
                height,
                image::ExtendedColorType::Rgba8,
            )
            .unwrap();
        bytes
    }

    #[test]
    fn canonical_bytes_are_the_preview_and_the_model_input() {
        let image = ImageInput::prepare(&png(24, 16, Rgba([0, 255, 0, 255]))).unwrap();
        assert_eq!(image.info().width, 24);
        assert_eq!(image.info().height, 16);
        assert_eq!(image.info().mime_type, "image/jpeg");
        assert_eq!(image.info().bytes, image.bytes().len());
        assert_eq!(
            image.info().sha256,
            format!("{:x}", Sha256::digest(image.bytes()))
        );
        let message = image.message_content(&json!({"question": "What happened?"}));
        assert_eq!(message[0]["text"], r#"{"question":"What happened?"}"#);
        assert_eq!(message[1]["image_url"]["url"], image.data_url());
        let encoded = image.data_url();
        assert_eq!(
            STANDARD.decode(encoded.split_once(',').unwrap().1).unwrap(),
            image.bytes()
        );
        assert!(!format!("{image:?}").contains("base64"));
    }

    #[test]
    fn transparent_pixels_become_white_before_resizing() {
        let image = ImageInput::prepare(&png(4, 4, Rgba([7, 23, 91, 0]))).unwrap();
        let decoded = image::load_from_memory(image.bytes()).unwrap().into_rgb8();
        assert!(decoded
            .pixels()
            .all(|pixel| pixel.0.iter().all(|v| *v >= 250)));
    }

    #[test]
    fn wide_images_are_reduced_without_changing_aspect_ratio() {
        let image = ImageInput::prepare(&png(3200, 800, Rgba([20, 30, 40, 255]))).unwrap();
        assert_eq!((image.info().width, image.info().height), (1600, 400));
        assert!(image.info().bytes <= MAX_OUTPUT_BYTES);
    }

    #[test]
    fn unsupported_truncated_and_oversize_inputs_are_refused() {
        assert_eq!(
            ImageInput::prepare(b"<svg/>").unwrap_err(),
            ImageError::Unsupported
        );
        assert_eq!(
            ImageInput::prepare(b"GIF89a").unwrap_err(),
            ImageError::Unsupported
        );
        assert_eq!(
            ImageInput::prepare(&vec![0; MAX_INPUT_BYTES + 1]).unwrap_err(),
            ImageError::TooLarge
        );
        let mut image = png(20, 20, Rgba([0, 0, 0, 255]));
        assert!(ImageInput::prepare(&image[..image.len() - 12]).is_err());
        image.truncate(40);
        assert!(ImageInput::prepare(&image).is_err());
        assert_eq!(
            ImageInput::prepare_base64("not base64").unwrap_err(),
            ImageError::Invalid
        );
        assert_eq!(
            ImageInput::prepare_base64(&"a".repeat(MAX_INPUT_BYTES.div_ceil(3) * 4 + 1))
                .unwrap_err(),
            ImageError::TooLarge
        );
        let jpeg = ImageInput::prepare(&png(8, 8, Rgba([0, 0, 0, 255]))).unwrap();
        assert!(
            ImageInput::prepare(&jpeg.bytes()[..jpeg.bytes().len() - 2]).is_err(),
            "an image without its JPEG end marker is truncated"
        );
    }

    #[test]
    fn jpeg_metadata_is_not_copied_to_the_prepared_image() {
        let image = ImageInput::prepare(&png(16, 8, Rgba([100, 80, 60, 255]))).unwrap();
        let comment = b"private-user C:\\Users\\private-user\\photo.jpg";
        let mut with_comment = image.bytes()[..2].to_vec();
        with_comment.extend([0xff, 0xfe]);
        with_comment.extend(((comment.len() + 2) as u16).to_be_bytes());
        with_comment.extend(comment);
        with_comment.extend(&image.bytes()[2..]);
        let prepared = ImageInput::prepare(&with_comment).unwrap();
        assert!(!prepared
            .bytes()
            .windows(comment.len())
            .any(|w| w == comment));
        let decoder = image::codecs::jpeg::JpegDecoder::new(Cursor::new(prepared.bytes()));
        assert!(decoder.unwrap().exif_metadata().unwrap().is_none());
    }

    #[test]
    fn exif_rotation_is_applied_before_metadata_is_removed() {
        let source = ImageInput::prepare(&png(16, 8, Rgba([100, 80, 60, 255]))).unwrap();
        let exif = [
            b'E', b'x', b'i', b'f', 0, 0, b'I', b'I', 42, 0, 8, 0, 0, 0, 1, 0, 0x12, 0x01, 3, 0, 1,
            0, 0, 0, 6, 0, 0, 0, 0, 0, 0, 0,
        ];
        let mut input = source.bytes()[..2].to_vec();
        input.extend([0xff, 0xe1]);
        input.extend(((exif.len() + 2) as u16).to_be_bytes());
        input.extend(exif);
        input.extend(&source.bytes()[2..]);
        let prepared = ImageInput::prepare(&input).unwrap();
        assert_eq!((prepared.info().width, prepared.info().height), (8, 16));
        let mut decoder =
            image::codecs::jpeg::JpegDecoder::new(Cursor::new(prepared.bytes())).unwrap();
        assert!(decoder.exif_metadata().unwrap().is_none());
    }

    #[test]
    fn animation_and_dimension_bombs_are_refused() {
        let mut image = png(4, 4, Rgba([0, 0, 0, 255]));
        let mut chunk = b"acTL".to_vec();
        chunk.extend(2_u32.to_be_bytes());
        chunk.extend(0_u32.to_be_bytes());
        let mut crc = u32::MAX;
        for byte in &chunk {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = (crc >> 1) ^ (0xedb88320 & 0_u32.wrapping_sub(crc & 1));
            }
        }
        let mut animation = 8_u32.to_be_bytes().to_vec();
        animation.extend(chunk);
        animation.extend((!crc).to_be_bytes());
        image.splice(33..33, animation);
        assert_eq!(
            ImageInput::prepare(&image).unwrap_err(),
            ImageError::Animated
        );
        assert_eq!(
            ImageInput::prepare(&png(16_385, 1, Rgba([0, 0, 0, 255]))).unwrap_err(),
            ImageError::TooLarge
        );
        assert_eq!(
            ImageInput::prepare(&png(5000, 4801, Rgba([0, 0, 0, 255]))).unwrap_err(),
            ImageError::TooLarge
        );
    }

    struct Capture {
        id: &'static str,
        messages: Vec<Value>,
        image_turn: bool,
    }

    impl Model for Capture {
        fn complete(
            &mut self,
            messages: &[Value],
            _: &Value,
            _: ToolChoice,
            _: Duration,
        ) -> Result<Reply, AgentError> {
            self.messages = messages.to_vec();
            Err(AgentError::Cancelled)
        }
        fn model_id(&self) -> Option<&str> {
            Some(self.id)
        }
        fn try_escalate(
            &mut self,
            _: crate::routing::EscalationReason,
            _: usize,
        ) -> Option<crate::routing::ModelChange> {
            assert!(!self.image_turn, "image turns never attempt escalation");
            None
        }
    }

    struct NoTools(Sharing);
    impl Toolbox for NoTools {
        fn specs(&self) -> &[ToolSpec] {
            &[]
        }
        fn run(&mut self, _: &ToolSpec, _: &Args) -> ToolOutput {
            panic!("no tools declared")
        }
        fn sharing(&self) -> Sharing {
            self.0
        }
    }

    #[test]
    fn image_turn_uses_the_exact_preview_without_recording_pixels_as_evidence() {
        let image = ImageInput::prepare(&png(8, 8, Rgba([0, 100, 0, 255]))).unwrap();
        let snapshot = Snapshot::empty("image-test");
        let privacy = PrivacyContext::default();
        let expected = conversation_message_with_privacy(
            &[],
            "What happened?",
            &snapshot,
            Sharing::Complete,
            &privacy,
        );
        let mut model = Capture {
            id: MODEL_ID,
            messages: Vec::new(),
            image_turn: true,
        };
        let result = converse_with_image(
            &mut model,
            &[],
            "What happened?",
            &snapshot,
            &mut NoTools(Sharing::Complete),
            &privacy,
            &image,
            |_| {},
            |_, _| false,
        );
        assert_eq!(result.error, Some(AgentError::Cancelled));
        assert_eq!(
            model.messages[1]["content"],
            image.message_content(&expected)
        );
        assert!(model.messages[0]["content"]
            .as_str()
            .unwrap()
            .contains("never O0"));
        assert!(!serde_json::to_string(&result).unwrap().contains("base64"));
        let past = [PastTurn {
            question: "What happened?".into(),
            answer: "A screenshot was attached.".into(),
            ..PastTurn::default()
        }];
        model.image_turn = false;
        converse_with_privacy(
            &mut model,
            &past,
            "And now?",
            &snapshot,
            &mut NoTools(Sharing::Complete),
            &privacy,
            |_| {},
            |_, _| false,
        );
        assert!(model.messages[1]["content"].is_string());
        assert!(!serde_json::to_string(&model.messages)
            .unwrap()
            .contains("data:image"));
    }

    #[test]
    fn anonymized_sharing_and_nonvision_models_make_no_provider_request() {
        let image = ImageInput::prepare(&png(8, 8, Rgba([0, 100, 0, 255]))).unwrap();
        for (sharing, id) in [
            (Sharing::Anonymized, MODEL_ID),
            (Sharing::Complete, "nvidia/Nemotron-3-Super"),
        ] {
            let mut model = Capture {
                id,
                messages: Vec::new(),
                image_turn: true,
            };
            let result = converse_with_image(
                &mut model,
                &[],
                "Question",
                &Snapshot::empty("image-test"),
                &mut NoTools(sharing),
                &PrivacyContext::default(),
                &image,
                |_| {},
                |_, _| false,
            );
            assert_eq!(result.error, Some(AgentError::ImageNotAllowed));
            assert!(model.messages.is_empty());
        }
    }

    #[test]
    fn tool_context_compaction_does_not_touch_the_image_or_its_text() {
        let image = ImageInput::prepare(&png(8, 8, Rgba([0, 100, 0, 255]))).unwrap();
        let mut messages = vec![
            json!({"role":"user","content":image.message_content(&json!({"question":"Look"}))}),
            json!({"role":"tool","content":json!({"data":"x".repeat(40_000)}).to_string()}),
            json!({"role":"tool","content":"{}"}),
        ];
        let first = messages[0].clone();
        crate::tool_context::ToolContext::default().compact(&mut messages);
        assert_eq!(messages[0], first);
    }
}
