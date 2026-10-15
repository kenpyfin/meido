//! Images attached to one inbound user message, forwarded to the model as vision blocks.
//!
//! Every channel collects attachments into [`InboundImages`] and hands it to the shared agent
//! entry point. Images past the caps are not forwarded; they stay on disk and in the
//! `[document]` attachment notes, so tools can still open them by path.

use base64::Engine as _;
use tracing::warn;

use crate::claude::{ContentBlock, ImageSource, Message, MessageContent};

/// Most images forwarded with one message.
pub const MAX_INBOUND_IMAGES: usize = 6;

/// Combined base64 size of forwarded images. Gemini rejects requests whose inline data exceeds
/// 20 MB. The first image is always accepted so a single large photo behaves as before.
pub const MAX_INBOUND_IMAGES_BASE64_BYTES: usize = 20 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundImage {
    pub base64: String,
    pub media_type: String,
}

#[derive(Debug, Clone, Default)]
pub struct InboundImages {
    images: Vec<InboundImage>,
    base64_bytes: usize,
    dropped: usize,
}

fn is_image_media_type(media_type: &str) -> bool {
    media_type.trim().to_ascii_lowercase().starts_with("image/")
}

fn base64_len(raw_len: usize) -> usize {
    raw_len.div_ceil(3).saturating_mul(4)
}

impl InboundImages {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn single(base64: String, media_type: String) -> Self {
        let mut images = Self::new();
        images.push_base64(base64, media_type);
        images
    }

    pub fn is_empty(&self) -> bool {
        self.images.is_empty()
    }

    pub fn len(&self) -> usize {
        self.images.len()
    }

    /// Images that were attachments of image type but exceeded the caps.
    pub fn dropped(&self) -> usize {
        self.dropped
    }

    pub fn images(&self) -> &[InboundImage] {
        &self.images
    }

    /// True when an image of `base64_len` bytes fits under the count and size caps.
    fn fits(&self, base64_len: usize) -> bool {
        if self.images.len() >= MAX_INBOUND_IMAGES {
            return false;
        }
        self.images.is_empty()
            || self.base64_bytes.saturating_add(base64_len) <= MAX_INBOUND_IMAGES_BASE64_BYTES
    }

    /// True when an image of `raw_len` bytes would be accepted. Lets callers skip reading or
    /// encoding a file that would be dropped anyway.
    pub fn would_accept(&self, media_type: &str, raw_len: usize) -> bool {
        is_image_media_type(media_type) && self.fits(base64_len(raw_len))
    }

    /// Counts an image the caller skipped after [`Self::would_accept`] returned false.
    pub fn record_dropped(&mut self, media_type: &str) {
        if is_image_media_type(media_type) {
            self.dropped += 1;
        }
    }

    /// Adds an already-encoded image. Non-image media types are ignored; images over the caps
    /// are counted in [`Self::dropped`]. Returns whether the image was kept.
    pub fn push_base64(&mut self, base64: String, media_type: String) -> bool {
        if !is_image_media_type(&media_type) {
            return false;
        }
        if !self.fits(base64.len()) {
            self.dropped += 1;
            return false;
        }
        self.base64_bytes = self.base64_bytes.saturating_add(base64.len());
        self.images.push(InboundImage { base64, media_type });
        true
    }

    /// Encodes and adds raw image bytes. See [`Self::push_base64`].
    pub fn push_bytes(&mut self, bytes: &[u8], media_type: &str) -> bool {
        if !is_image_media_type(media_type) {
            return false;
        }
        if !self.fits(base64_len(bytes.len())) {
            self.dropped += 1;
            return false;
        }
        let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
        self.push_base64(encoded, media_type.to_string())
    }

    /// Prepends the images to the trailing user message, keeping its text after them.
    /// Does nothing when there are no images or the last message is not from the user.
    pub fn attach_to_last_user_message(self, messages: &mut [Message]) {
        if self.dropped > 0 {
            warn!(
                forwarded = self.images.len(),
                dropped = self.dropped,
                max_images = MAX_INBOUND_IMAGES,
                "Inbound images over cap were not forwarded to the model"
            );
        }
        if self.images.is_empty() {
            return;
        }
        let Some(last_msg) = messages.last_mut() else {
            return;
        };
        if last_msg.role != "user" {
            return;
        }
        let text_content = match &last_msg.content {
            MessageContent::Text(t) => t.clone(),
            _ => String::new(),
        };
        let mut blocks: Vec<ContentBlock> = self
            .images
            .into_iter()
            .map(|img| ContentBlock::Image {
                source: ImageSource {
                    source_type: "base64".into(),
                    media_type: img.media_type,
                    data: img.base64,
                },
            })
            .collect();
        if !text_content.is_empty() {
            blocks.push(ContentBlock::Text { text: text_content });
        }
        last_msg.content = MessageContent::Blocks(blocks);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user_text(text: &str) -> Message {
        Message {
            role: "user".into(),
            content: MessageContent::Text(text.into()),
        }
    }

    #[test]
    fn keeps_every_image_up_to_the_count_cap() {
        let mut images = InboundImages::new();
        for _ in 0..MAX_INBOUND_IMAGES + 2 {
            images.push_bytes(b"img", "image/png");
        }
        assert_eq!(images.len(), MAX_INBOUND_IMAGES);
        assert_eq!(images.dropped(), 2);
    }

    #[test]
    fn ignores_non_image_attachments() {
        let mut images = InboundImages::new();
        assert!(!images.push_bytes(b"%PDF", "application/pdf"));
        assert!(images.is_empty());
        assert_eq!(images.dropped(), 0);
    }

    #[test]
    fn first_image_is_kept_even_over_the_size_budget() {
        let mut images = InboundImages::new();
        let big = "A".repeat(MAX_INBOUND_IMAGES_BASE64_BYTES + 4);
        assert!(images.push_base64(big, "image/jpeg".into()));
        assert!(!images.push_base64("AAAA".into(), "image/png".into()));
        assert_eq!(images.len(), 1);
        assert_eq!(images.dropped(), 1);
    }

    #[test]
    fn would_accept_matches_push_decision() {
        let mut images = InboundImages::new();
        images.push_base64(
            "A".repeat(MAX_INBOUND_IMAGES_BASE64_BYTES - 8),
            "image/jpeg".into(),
        );
        assert!(images.would_accept("image/png", 6));
        assert!(!images.would_accept("image/png", 7));
        assert!(!images.would_accept("text/plain", 1));
    }

    #[test]
    fn attaches_all_images_before_text_in_order() {
        let mut images = InboundImages::new();
        images.push_base64("AAAA".into(), "image/jpeg".into());
        images.push_base64("BBBB".into(), "image/png".into());
        let mut messages = vec![user_text("combine these")];
        images.attach_to_last_user_message(&mut messages);

        let MessageContent::Blocks(blocks) = &messages[0].content else {
            panic!("expected blocks");
        };
        assert_eq!(blocks.len(), 3);
        assert!(matches!(
            &blocks[0],
            ContentBlock::Image { source } if source.media_type == "image/jpeg"
        ));
        assert!(matches!(
            &blocks[1],
            ContentBlock::Image { source } if source.media_type == "image/png"
        ));
        assert!(matches!(&blocks[2], ContentBlock::Text { text } if text == "combine these"));
    }

    #[test]
    fn leaves_non_user_trailing_message_untouched() {
        let mut messages = vec![Message {
            role: "assistant".into(),
            content: MessageContent::Text("hi".into()),
        }];
        InboundImages::single("AAAA".into(), "image/png".into())
            .attach_to_last_user_message(&mut messages);
        assert!(matches!(messages[0].content, MessageContent::Text(_)));
    }
}
