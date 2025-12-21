//! XML writer for danmu messages.
//!
//! Provides functionality to write danmu messages to XML files in a Bilibili-compatible format.
//!
//! ## Danmu Format
//!
//! Each danmu element has the following structure:
//! ```xml
//! <d p="{time},{type},{size},{color},{timestamp},{pool},{uid_crc32},{row_id} user={username}">{Text}</d>
//! ```
//!
//! Where:
//! - `time`: Time offset in the video (seconds with 3 decimal places)
//! - `type`: Danmu display type (1=scroll, 4=bottom, 5=top, etc.)
//! - `size`: Font size (default: 25)
//! - `color`: Decimal RGB color (default: 16777215 = white)
//! - `timestamp`: Unix timestamp when danmu was sent
//! - `pool`: Danmu pool (0=normal, 1=subtitle, 2=special)
//! - `uid_crc32`: CRC32 hash of the sender's user ID
//! - `row_id`: Row ID for ordering (uses message count)

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use tokio::fs::File;
use tokio::io::AsyncWriteExt;

use crate::danmaku::error::Result;
use crate::danmaku::message::{DanmuMessage, DanmuType};

/// Default font size for danmu display.
const DEFAULT_FONT_SIZE: u32 = 25;

/// Default color (white) as decimal RGB.
const DEFAULT_COLOR: u32 = 16777215;

/// Default danmu pool (normal pool).
const DEFAULT_POOL: u8 = 0;

/// XML writer for danmu messages.
///
/// This writer creates XML files in Bilibili-compatible format suitable for
/// video players that support danmu overlay. Timestamps are written as offsets
/// from the segment start time, making them suitable for video synchronization.
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
    /// The start time of the current segment.
    /// Timestamps are written as second offsets from this time.
    segment_start_time: DateTime<Utc>,
    /// Optional header comments (metadata).
    header_comments: Vec<String>,
}

impl XmlDanmuWriter {
    /// Create a new XML writer at the specified path.
    ///
    /// This will create the file and write the XML header.
    /// The segment start time is set to the current time.
    pub async fn new(path: &Path) -> Result<Self> {
        Self::with_start_time_and_comments(path, Utc::now(), Vec::new()).await
    }

    /// Create a new XML writer with a specific segment start time.
    ///
    /// This is useful when the segment start time is known
    pub async fn with_start_time(path: &Path, segment_start_time: DateTime<Utc>) -> Result<Self> {
        Self::with_start_time_and_comments(path, segment_start_time, Vec::new()).await
    }

    /// Create a new XML writer with a specific segment start time and header comments.
    pub async fn with_start_time_and_comments(
        path: &Path,
        segment_start_time: DateTime<Utc>,
        header_comments: Vec<String>,
    ) -> Result<Self> {
        let file = File::create(path).await?;
        let mut writer = Self {
            path: path.to_path_buf(),
            file: Some(file),
            message_count: 0,
            segment_start_time,
            header_comments,
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

    /// Get the segment start time.
    pub fn segment_start_time(&self) -> DateTime<Utc> {
        self.segment_start_time
    }

    async fn write_header(&mut self) -> Result<()> {
        if let Some(file) = &mut self.file {
            file.write_all(b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n")
                .await?;
            for comment in &self.header_comments {
                let comment_xml = format!("<!-- {} -->\n", comment.replace("-->", "--"));
                file.write_all(comment_xml.as_bytes()).await?;
            }
            file.write_all(b"<i>\n").await?;
        }
        Ok(())
    }

    /// Write a danmu message to the XML file.
    ///
    /// The format follows Bilibili's danmu XML format:
    /// `<d p="{time},{type},{size},{color},{timestamp},{pool},{uid_crc32},{row_id},user={username}">{content}</d>`
    ///
    /// - `time`: Seconds offset from segment start (3 decimal places)
    /// - `type`: Danmu type (1=scroll right-to-left, 4=bottom, 5=top)
    /// - `size`: Font size (default 25)
    /// - `color`: Decimal RGB color (default white = 16777215)
    /// - `timestamp`: Unix timestamp of the message
    /// - `pool`: Danmu pool (0=normal)
    /// - `uid_crc32`: CRC32 of user ID
    /// - `row_id`: Message sequence number
    /// - `user`: Username of the sender
    pub async fn write_message(&mut self, message: &DanmuMessage) -> Result<()> {
        if let Some(file) = &mut self.file {
            // Calculate offset from segment start in seconds (3 decimal places)
            let offset_ms = (message.timestamp - self.segment_start_time)
                .num_milliseconds()
                .max(0);
            let offset_secs = offset_ms as f64 / 1000.0;

            // Get danmu type for Bilibili format
            let danmu_type = message_type_to_bilibili_type(&message.message_type);

            // Calculate CRC32 of user ID
            let uid_crc32 = crc32_hash(&message.user_id);

            // Unix timestamp
            let unix_timestamp = message.timestamp.timestamp();

            // Row ID is the message count + 1
            let row_id = self.message_count + 1;

            // Format: <d p="{time},{type},{size},{color},{timestamp},{pool},{uid_crc32},{row_id},user={username}">{content}</d>
            let xml = format!(
                "  <d p=\"{:.3},{},{},{},{},{},{},{} user={}\">{}</d>\n",
                offset_secs,
                danmu_type,
                DEFAULT_FONT_SIZE,
                DEFAULT_COLOR,
                unix_timestamp,
                DEFAULT_POOL,
                uid_crc32,
                row_id,
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
            file.write_all(b"</i>\n").await?;
            file.flush().await?;
        }
        self.file = None;
        Ok(())
    }
}

/// Convert a message type to Bilibili danmu type.
///
/// Bilibili danmu types:
/// - 1: Scroll (right to left)
/// - 4: Bottom fixed
/// - 5: Top fixed
/// - 6: Reverse scroll (left to right)
/// - 7: Special
/// - 8: Advanced
pub fn message_type_to_bilibili_type(msg_type: &DanmuType) -> u8 {
    match msg_type {
        DanmuType::Chat => 1,         // Regular chat = scroll
        DanmuType::Gift => 1,         // Gift = scroll
        DanmuType::SuperChat => 5,    // SuperChat = top fixed (prominent)
        DanmuType::System => 4,       // System = bottom fixed
        DanmuType::UserJoin => 1,     // User join = scroll
        DanmuType::Follow => 1,       // Follow = scroll
        DanmuType::Subscription => 5, // Subscription = top fixed
        DanmuType::Other => 1,        // Other = scroll
    }
}

/// Convert a message type to its integer representation (legacy format).
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

/// Calculate CRC32 hash of a string.
///
/// Uses the standard CRC32 (IEEE) polynomial.
pub fn crc32_hash(s: &str) -> u32 {
    // CRC32 lookup table (IEEE polynomial 0xEDB88320)
    const CRC32_TABLE: [u32; 256] = generate_crc32_table();

    let mut crc: u32 = 0xFFFFFFFF;
    for byte in s.bytes() {
        let index = ((crc ^ (byte as u32)) & 0xFF) as usize;
        crc = (crc >> 8) ^ CRC32_TABLE[index];
    }
    !crc
}

/// Generate CRC32 lookup table at compile time.
const fn generate_crc32_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let polynomial: u32 = 0xEDB88320;

    let mut i = 0;
    while i < 256 {
        let mut crc = i as u32;
        let mut j = 0;
        while j < 8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ polynomial;
            } else {
                crc >>= 1;
            }
            j += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
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
    fn test_message_type_to_bilibili_type() {
        assert_eq!(message_type_to_bilibili_type(&DanmuType::Chat), 1);
        assert_eq!(message_type_to_bilibili_type(&DanmuType::Gift), 1);
        assert_eq!(message_type_to_bilibili_type(&DanmuType::SuperChat), 5);
        assert_eq!(message_type_to_bilibili_type(&DanmuType::System), 4);
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

    #[test]
    fn test_crc32_hash() {
        // Test known CRC32 values
        assert_eq!(crc32_hash(""), 0);
        assert_eq!(crc32_hash("123456"), 0x0972D361);
        assert_eq!(crc32_hash("test"), 0xD87F7E0C);
    }
}
