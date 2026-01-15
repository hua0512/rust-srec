//! XML writer for danmu messages.
//!
//! Provides functionality to write danmu messages to XML files in a mostly
//! Bilibili-compatible format.
//!
//! This writer uses the standard `<d>` nodes for regular danmu messages, and
//! additionally emits custom `<gift>` / `<sc>` nodes for gift and super chat
//! events (for richer downstream processing).
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
//! - `timestamp`: Unix timestamp in milliseconds when danmu was sent
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
            self.header_comments.clear();
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
    /// - `timestamp`: Unix timestamp in milliseconds of the message
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

            // Calculate CRC32 of user ID
            let uid_crc32 = crc32_hash(&message.user_id);

            // Unix timestamp in milliseconds
            let unix_timestamp_ms = message.timestamp.timestamp_millis();

            // Row ID is the message count + 1
            let row_id = self.message_count + 1;

            let xml = match message.message_type {
                DanmuType::Gift => gift_to_xml(message, offset_secs, unix_timestamp_ms),
                DanmuType::SuperChat => super_chat_to_xml(message, offset_secs, unix_timestamp_ms),
                _ => {
                    // Get danmu type for Bilibili format
                    let danmu_type = message_type_to_bilibili_type(&message.message_type);
                    let color =
                        message_color_to_bilibili_color(message).unwrap_or(DEFAULT_COLOR);
                    let content = message_content_for_xml(message);

                    // Format: <d p="{time},{type},{size},{color},{timestamp},{pool},{uid_crc32},{row_id}" user="{username}">{content}</d>
                    format!(
                        "  <d p=\"{:.3},{},{},{},{},{},{},{}\" user=\"{}\">{}</d>\n",
                        offset_secs,
                        danmu_type,
                        DEFAULT_FONT_SIZE,
                        color,
                        unix_timestamp_ms,
                        DEFAULT_POOL,
                        uid_crc32,
                        row_id,
                        escape_xml(&message.username),
                        escape_xml(&content),
                    )
                }
            };
            file.write_all(xml.as_bytes()).await?;
            self.message_count += 1;

            // Flush periodically
            if self.message_count.is_multiple_of(100) {
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

fn message_color_to_bilibili_color(message: &DanmuMessage) -> Option<u32> {
    let raw = message.color.as_deref()?.trim();
    if raw.is_empty() {
        return None;
    }

    let hex = raw.strip_prefix('#').unwrap_or(raw);
    if hex.len() != 6 || !hex.is_ascii() {
        return None;
    }

    u32::from_str_radix(hex, 16).ok()
}

fn message_content_for_xml(message: &DanmuMessage) -> String {
    match message.message_type {
        DanmuType::SuperChat => {
            let content = message.content.trim();
            let content = content.to_string();

            let price = message
                .metadata
                .as_ref()
                .and_then(|m| m.get("price"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);

            if price > 0 && !content.is_empty() {
                format!("[SC ￥{}] {}", price, content)
            } else if price > 0 {
                format!("[SC ￥{}]", price)
            } else {
                content
            }
        }
        DanmuType::Gift => {
            let content = message.content.trim();
            if !content.is_empty() {
                return content.to_string();
            }

            let Some(metadata) = message.metadata.as_ref() else {
                return String::new();
            };

            let gift_name = metadata
                .get("gift_name")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let gift_count = metadata
                .get("gift_count")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);

            if gift_count > 0 && !gift_name.is_empty() {
                format!("赠送 {} x{}", gift_name, gift_count)
            } else {
                String::new()
            }
        }
        _ => message.content.clone(),
    }
}

fn gift_to_xml(message: &DanmuMessage, ts: f64, timestamp_ms: i64) -> String {
    let mut gift_name = "";
    let mut gift_count: u64 = 0;
    let mut price: u64 = 0;

    if let Some(metadata) = message.metadata.as_ref() {
        gift_name = metadata
            .get("gift_name")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        gift_count = metadata
            .get("gift_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        price = metadata
            .get("price")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
    }

    format!(
        "  <gift ts=\"{:.3}\" giftname=\"{}\" giftcount=\"{}\" price=\"{}\" user=\"{}\" uid=\"{}\" timestamp=\"{}\"></gift>\n",
        ts,
        escape_xml(gift_name),
        gift_count,
        price,
        escape_xml(&message.username),
        escape_xml(&message.user_id),
        timestamp_ms,
    )
}

fn super_chat_to_xml(message: &DanmuMessage, ts: f64, timestamp_ms: i64) -> String {
    let mut price: u64 = 0;
    let mut keep_time: u64 = 0;

    if let Some(metadata) = message.metadata.as_ref() {
        price = metadata
            .get("price")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        keep_time = metadata
            .get("keep_time")
            .or_else(|| metadata.get("time"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
    }

    format!(
        "  <sc ts=\"{:.3}\" user=\"{}\" uid=\"{}\" price=\"{}\" time=\"{}\" timestamp=\"{}\">{}</sc>\n",
        ts,
        escape_xml(&message.username),
        escape_xml(&message.user_id),
        price,
        keep_time,
        timestamp_ms,
        escape_xml(message.content.trim()),
    )
}

/// Escape special XML characters in a string.
pub fn escape_xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
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

    #[tokio::test]
    async fn test_xml_writer_writes_gift_and_super_chat_content() {
        use chrono::TimeZone;

        let tmp = std::env::temp_dir()
            .join(format!("rust-srec-xml-writer-{}.xml", uuid::Uuid::new_v4()));
        let start = Utc.timestamp_opt(1_700_000_000, 0).single().unwrap();
        let mut writer =
            XmlDanmuWriter::with_start_time(&tmp, start).await.expect("writer");

        let gift = DanmuMessage::gift("g1", "u1", "GiftUser", "Rocket", 5)
            .with_timestamp(start + chrono::Duration::milliseconds(1500))
            .with_color("#FF0000");
        writer.write_message(&gift).await.expect("gift write");

        let super_chat = DanmuMessage::super_chat("s1", "u2", "SCUser", "Hello", 30)
            .with_timestamp(start + chrono::Duration::milliseconds(2500));
        writer.write_message(&super_chat).await.expect("sc write");

        writer.finalize().await.expect("finalize");

        let xml = tokio::fs::read_to_string(&tmp).await.expect("read xml");
        let _ = tokio::fs::remove_file(&tmp).await;

        assert!(xml.contains("<gift "));
        assert!(xml.contains("giftname=\"Rocket\""));
        assert!(xml.contains("giftcount=\"5\""));
        assert!(xml.contains("user=\"GiftUser\""));
        assert!(xml.contains("uid=\"u1\""));
        assert!(xml.contains("ts=\"1.500\""));
        assert!(xml.contains("timestamp=\""));

        assert!(xml.contains("<sc "));
        assert!(xml.contains("user=\"SCUser\""));
        assert!(xml.contains("uid=\"u2\""));
        assert!(xml.contains("price=\"30\""));
        assert!(xml.contains("ts=\"2.500\""));
        assert!(xml.contains(">Hello</sc>"));
    }
}
