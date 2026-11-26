//! Output reader utilities for child process monitoring.
//!
//! FFmpeg writes periodic progress updates using carriage returns (`\r`) to rewrite the same
//! terminal line. When stdout/stderr are piped, those `\r` updates still occur but are not
//! newline-delimited, so `BufReadExt::lines()` may not surface them in a timely manner.
//!
//! This module provides an async reader that yields "records" delimited by either `\n` or `\r`.

use std::io;

use tokio::io::{AsyncRead, BufReader};

/// Reads an async stream and yields text records delimited by `\n` or `\r`.
pub struct OutputRecordReader<R> {
    reader: BufReader<R>,
    pending: Vec<u8>,
    scratch: [u8; 4096],
    scanned: usize,
    #[cfg(test)]
    scanned_bytes: usize,
}

impl<R> OutputRecordReader<R>
where
    R: AsyncRead + Unpin,
{
    pub fn new(reader: R) -> Self {
        Self {
            reader: BufReader::new(reader),
            pending: Vec::new(),
            scratch: [0u8; 4096],
            scanned: 0,
            #[cfg(test)]
            scanned_bytes: 0,
        }
    }

    /// Returns the next record from the stream.
    ///
    /// Records are delimited by either `\n` or `\r`. Consecutive delimiters are skipped.
    pub async fn next_record(&mut self) -> io::Result<Option<String>> {
        loop {
            let delimiter = find_record_delimiter(&self.pending[self.scanned..]);
            #[cfg(test)]
            {
                self.scanned_bytes +=
                    delimiter.map_or(self.pending.len() - self.scanned, |(index, _)| index + 1);
            }
            if let Some((index, _delim)) = delimiter {
                let idx = self.scanned + index;
                let record = String::from_utf8_lossy(&self.pending[..idx])
                    .trim()
                    .to_string();
                let delimiter_count = self.pending[idx..]
                    .iter()
                    .take_while(|&&byte| matches!(byte, b'\r' | b'\n'))
                    .count();
                self.pending.drain(..idx + delimiter_count);
                self.scanned = 0;
                if record.is_empty() {
                    continue;
                }
                return Ok(Some(record));
            }

            // Preserve this position across pending reads and cancelled calls:
            // bytes without delimiters never need to be examined again.
            self.scanned = self.pending.len();

            let n = tokio::io::AsyncReadExt::read(&mut self.reader, &mut self.scratch).await?;
            if n == 0 {
                if self.pending.is_empty() {
                    return Ok(None);
                }

                let record = String::from_utf8_lossy(&self.pending).trim().to_string();
                self.pending.clear();
                self.scanned = 0;

                if record.is_empty() {
                    return Ok(None);
                }
                return Ok(Some(record));
            }

            self.pending.extend_from_slice(&self.scratch[..n]);
        }
    }
}

fn find_record_delimiter(buf: &[u8]) -> Option<(usize, u8)> {
    buf.iter()
        .enumerate()
        .find_map(|(idx, &b)| matches!(b, b'\n' | b'\r').then_some((idx, b)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    struct Fragmented {
        bytes: std::io::Cursor<Vec<u8>>,
        chunk: usize,
    }

    impl AsyncRead for Fragmented {
        fn poll_read(
            mut self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            buffer: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<io::Result<()>> {
            let start = self.bytes.position() as usize;
            let end = (start + self.chunk.min(buffer.remaining())).min(self.bytes.get_ref().len());
            buffer.put_slice(&self.bytes.get_ref()[start..end]);
            self.bytes.set_position(end as u64);
            std::task::Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn fragmented_long_record_is_scanned_once_and_preserves_records() {
        let long = "中😀".repeat(4096);
        let input = format!(" {long} \r\n\r tail\nlast").into_bytes();
        let mut reader = OutputRecordReader::new(Fragmented {
            bytes: std::io::Cursor::new(input.clone()),
            chunk: 1,
        });
        assert_eq!(reader.next_record().await.unwrap(), Some(long));
        assert_eq!(reader.next_record().await.unwrap().as_deref(), Some("tail"));
        assert_eq!(reader.next_record().await.unwrap().as_deref(), Some("last"));
        assert!(reader.next_record().await.unwrap().is_none());
        assert!(
            reader.scanned_bytes <= input.len(),
            "delimiter-free prefixes must not be rescanned"
        );
    }

    #[tokio::test]
    async fn cancelled_record_read_resumes_without_losing_buffered_utf8() {
        let (mut tx, rx) = tokio::io::duplex(64);
        tx.write_all(&[b'a', 0xe4, 0xb8]).await.unwrap();
        let mut reader = OutputRecordReader::new(rx);
        let mut read = Box::pin(reader.next_record());
        assert!(futures::poll!(read.as_mut()).is_pending());
        drop(read);
        tx.write_all(&[0xad, b'\r', b'\n', 0xff, b'\n'])
            .await
            .unwrap();
        drop(tx);
        assert_eq!(reader.next_record().await.unwrap().as_deref(), Some("a中"));
        assert_eq!(reader.next_record().await.unwrap().as_deref(), Some("�"));
        assert!(reader.next_record().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn splits_on_cr_and_lf() {
        let (mut tx, rx) = tokio::io::duplex(1024);

        tokio::spawn(async move {
            let _ = tx.write_all(b"one\rtwo\nthree\r\nfour").await;
        });

        let mut reader = OutputRecordReader::new(rx);
        let mut records = Vec::new();
        while let Some(line) = reader.next_record().await.unwrap() {
            records.push(line);
        }

        assert_eq!(records, vec!["one", "two", "three", "four"]);
    }
}
