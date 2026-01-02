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
        }
    }

    /// Returns the next record from the stream.
    ///
    /// Records are delimited by either `\n` or `\r`. Consecutive delimiters are skipped.
    pub async fn next_record(&mut self) -> io::Result<Option<String>> {
        loop {
            if let Some((idx, _delim)) = find_record_delimiter(&self.pending) {
                let record_bytes: Vec<u8> = self.pending.drain(..idx).collect();
                consume_delimiters(&mut self.pending);

                let record = String::from_utf8_lossy(&record_bytes).trim().to_string();
                if record.is_empty() {
                    continue;
                }
                return Ok(Some(record));
            }

            let n = tokio::io::AsyncReadExt::read(&mut self.reader, &mut self.scratch).await?;
            if n == 0 {
                if self.pending.is_empty() {
                    return Ok(None);
                }

                let record = String::from_utf8_lossy(&self.pending).trim().to_string();
                self.pending.clear();

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

fn consume_delimiters(buf: &mut Vec<u8>) {
    let n = buf
        .iter()
        .take_while(|&&b| matches!(b, b'\n' | b'\r'))
        .count();
    if n > 0 {
        buf.drain(..n);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

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
