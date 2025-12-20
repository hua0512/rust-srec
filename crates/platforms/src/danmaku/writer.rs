//! XML writer for danmu messages.
//!
//! Provides functionality to write danmu messages to XML files in a standard format.

use std::path::{Path, PathBuf};
use tokio::fs::File;
use tokio::io::AsyncWriteExt;

use crate::danmaku::error::Result;
use crate::danmaku::message::{DanmuMessage, DanmuType};

/// XML writer for danmu messages.
///
/// This writer creates XML files with a simple format suitable for storing
/// danmu/chat messages from live streams.
///
/// # Example
///
/// ```ignore
/// use platforms_parser::danmaku::XmlDanmuWriter;
/// use std::path::PathBuf;
///
/// let mut writer = XmlDanmuWriter::new(&PathBuf::from("output.xml")).await?;
/// writer.write_message(&message).await?;
/// writer.finalize().await?;
/// ```
pub struct XmlDanmuWriter {
    path: PathBuf,
    file: Option<File>,
    message_count: u64,
}

impl XmlDanmuWriter {
    /// Create a new XML writer at the specified path.
    ///
    /// This will create the file and write the XML header.
    pub async fn new(path: &Path) -> Result<Self> {
        let file = File::create(path).await?;
        let mut writer = Self {
            path: path.to_path_buf(),
            file: Some(file),
            message_count: 0,
        };

        // Write XML header
        writer.write_header().await?;

        Ok(writer)
    }

    /// Get the output path of this writer.
    pub fn output_path(&self) -> &Path {
        &self.path
    }

    /// Get the number of messages written so far.
    pub fn message_count(&self) -> u64 {
        self.message_count
    }

    async fn write_header(&mut self) -> Result<()> {
        if let Some(file) = &mut self.file {
            file.write_all(b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n")
                .await?;
            file.write_all(b"<danmu>\n").await?;
        }
        Ok(())
    }

    /// Write a danmu message to the XML file.
    pub async fn write_message(&mut self, message: &DanmuMessage) -> Result<()> {
        if let Some(file) = &mut self.file {
            let xml = format!(
                "  <d p=\"{},{},{},{}\">{}</d>\n",
                message.timestamp.timestamp_millis(),
                message_type_to_int(&message.message_type),
                escape_xml(&message.user_id),
                escape_xml(&message.username),
                escape_xml(&message.content),
            );
            file.write_all(xml.as_bytes()).await?;
            self.message_count += 1;

            // Flush periodically
            if self.message_count % 100 == 0 {
                file.flush().await?;
            }
        }
        Ok(())
    }

    /// Finalize the XML file by writing the closing tag.
    ///
    /// This should be called when all messages have been written.
    pub async fn finalize(&mut self) -> Result<()> {
        if let Some(file) = &mut self.file {
            file.write_all(b"</danmu>\n").await?;
            file.flush().await?;
        }
        self.file = None;
        Ok(())
    }
}

/// Convert a message type to its integer representation.
pub fn message_type_to_int(msg_type: &DanmuType) -> u8 {
    match msg_type {
        DanmuType::Chat => 1,
        DanmuType::Gift => 2,
        DanmuType::SuperChat => 3,
        DanmuType::System => 4,
        DanmuType::UserJoin => 5,
        DanmuType::Follow => 6,
        DanmuType::Subscription => 7,
        DanmuType::Other => 0,
    }
}

/// Escape special XML characters in a string.
pub fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escape_xml() {
        assert_eq!(escape_xml("hello"), "hello");
        assert_eq!(escape_xml("<script>"), "&lt;script&gt;");
        assert_eq!(escape_xml("a & b"), "a &amp; b");
        assert_eq!(escape_xml("\"quoted\""), "&quot;quoted&quot;");
    }

    #[test]
    fn test_message_type_to_int() {
        assert_eq!(message_type_to_int(&DanmuType::Chat), 1);
        assert_eq!(message_type_to_int(&DanmuType::Gift), 2);
        assert_eq!(message_type_to_int(&DanmuType::SuperChat), 3);
        assert_eq!(message_type_to_int(&DanmuType::System), 4);
        assert_eq!(message_type_to_int(&DanmuType::UserJoin), 5);
        assert_eq!(message_type_to_int(&DanmuType::Follow), 6);
        assert_eq!(message_type_to_int(&DanmuType::Subscription), 7);
        assert_eq!(message_type_to_int(&DanmuType::Other), 0);
    }
}
