//! Attachments the user adds to a turn. The UI reads the files and
//! downscales images; core only classifies them and builds the message
//! (see llm::build_user_message).

/// One attachment, already read into memory by the UI.
#[derive(Debug, Clone)]
pub enum Attachment {
    /// A text file, content inlined into the conversation.
    TextFile { name: String, content: String },
    /// An image, already downscaled to max 1024px on the long edge by
    /// the UI, as a `data:image/...;base64,...` URL.
    Image { name: String, data_url: String },
    /// Anything else (only the name is passed to the model).
    Other { name: String },
}

impl Attachment {
    /// Classify a file by extension and size. Text files are inlined
    /// only when small (<= 100 KiB); images are expected to arrive
    /// already downscaled from the UI. Returns None when the file
    /// should be treated as `Other`.
    pub fn classify(name: &str, bytes: &[u8]) -> AttachmentKind {
        let lower = name.to_ascii_lowercase();
        let ext = lower.rsplit('.').next().unwrap_or("");
        match ext {
            "jpg" | "jpeg" | "png" | "webp" | "gif" => AttachmentKind::Image,
            "txt" | "md" | "rs" | "py" | "js" | "ts" | "json" | "toml" | "yaml" | "yml" | "sh"
            | "csv" | "log" | "html" | "css" => {
                if bytes.len() <= 100 * 1024 {
                    AttachmentKind::Text
                } else {
                    AttachmentKind::Other
                }
            }
            _ => AttachmentKind::Other,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentKind {
    Text,
    Image,
    Other,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_by_extension() {
        assert_eq!(Attachment::classify("foto.jpg", &[]), AttachmentKind::Image);
        assert_eq!(
            Attachment::classify("note.md", b"ciao"),
            AttachmentKind::Text
        );
        assert_eq!(
            Attachment::classify("big.txt", &vec![b'x'; 200 * 1024]),
            AttachmentKind::Other
        );
        assert_eq!(Attachment::classify("app.zip", &[]), AttachmentKind::Other);
    }
}
