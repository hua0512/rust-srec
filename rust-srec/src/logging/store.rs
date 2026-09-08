//! Size/date rotation serialized with retention across processes sharing a directory.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{DateTime, NaiveDate, Utc};

const PREFIX: &str = "rust-srec.log.";
const LOCK_NAME: &str = ".rust-srec.log.lock";
const TRUNCATED: &[u8] = b"\n[log record truncated by byte limit]\n";
const RETENTION_DAYS: i64 = 7;

#[derive(Debug, Clone, Copy)]
pub(super) struct LogPolicy {
    pub(super) max_file_bytes: u64,
    pub(super) max_files: usize,
}

impl Default for LogPolicy {
    fn default() -> Self {
        Self {
            max_file_bytes: 16 * 1024 * 1024,
            max_files: 16,
        }
    }
}

impl LogPolicy {
    pub(super) fn from_env() -> crate::Result<Self> {
        Self::parse(
            std::env::var("LOG_MAX_FILE_BYTES")
                .map(Some)
                .or_else(|error| match error {
                    std::env::VarError::NotPresent => Ok(None),
                    _ => Err(crate::Error::config(
                        "LOG_MAX_FILE_BYTES must be valid Unicode",
                    )),
                })?
                .as_deref(),
            std::env::var("LOG_MAX_FILES")
                .map(Some)
                .or_else(|error| match error {
                    std::env::VarError::NotPresent => Ok(None),
                    _ => Err(crate::Error::config("LOG_MAX_FILES must be valid Unicode")),
                })?
                .as_deref(),
        )
    }

    fn parse(bytes: Option<&str>, files: Option<&str>) -> crate::Result<Self> {
        let defaults = Self::default();
        let max_file_bytes = match bytes {
            None => defaults.max_file_bytes,
            Some(value) => value.parse::<u64>().map_err(|_| {
                crate::Error::config(
                    "LOG_MAX_FILE_BYTES must be an integer between 1024 and 1073741824",
                )
            })?,
        };
        let max_files = match files {
            None => defaults.max_files,
            Some(value) => value.parse::<usize>().map_err(|_| {
                crate::Error::config("LOG_MAX_FILES must be an integer between 2 and 1024")
            })?,
        };
        if !(1024..=1024 * 1024 * 1024).contains(&max_file_bytes) {
            return Err(crate::Error::config(
                "LOG_MAX_FILE_BYTES must be between 1024 and 1073741824",
            ));
        }
        if !(2..=1024).contains(&max_files) {
            return Err(crate::Error::config(
                "LOG_MAX_FILES must be between 2 and 1024",
            ));
        }
        Ok(Self {
            max_file_bytes,
            max_files,
        })
    }
}

/// Both legacy daily files and numbered rotations have their UTC date in the name.
/// Other names remain outside managed retention, even if the API lists them.
pub(crate) fn managed_log_date(filename: &str) -> Option<NaiveDate> {
    parse_name(filename).map(|(date, _)| date)
}

fn parse_name(filename: &str) -> Option<(NaiveDate, u64)> {
    let suffix = filename.strip_prefix(PREFIX)?;
    let date_text = suffix.get(..10)?;
    let date = NaiveDate::parse_from_str(date_text, "%Y-%m-%d").ok()?;
    if date.format("%Y-%m-%d").to_string() != date_text {
        return None;
    }
    let index = match suffix.get(10..)? {
        "" => 0,
        rest => {
            let number = rest.strip_prefix('.')?;
            if number.len() != 20 || !number.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            let index = number.parse::<u64>().ok()?;
            if index == 0 {
                return None;
            }
            index
        }
    };
    Some((date, index))
}

fn filename(date: NaiveDate, index: u64) -> String {
    if index == 0 {
        format!("{PREFIX}{}", date.format("%Y-%m-%d"))
    } else {
        format!("{PREFIX}{}.{index:020}", date.format("%Y-%m-%d"))
    }
}

struct LogFile {
    path: PathBuf,
    date: NaiveDate,
    index: u64,
    bytes: u64,
}

pub(crate) struct LogStore {
    directory: PathBuf,
    policy: LogPolicy,
    truncated_records: AtomicU64,
    failed_writes: AtomicU64,
    #[cfg(test)]
    fail_removal: std::sync::atomic::AtomicBool,
    #[cfg(test)]
    reconcile_started: std::sync::atomic::AtomicBool,
}

impl LogStore {
    pub(super) fn new(directory: PathBuf, policy: LogPolicy) -> Self {
        Self {
            directory,
            policy,
            truncated_records: AtomicU64::new(0),
            failed_writes: AtomicU64::new(0),
            #[cfg(test)]
            fail_removal: std::sync::atomic::AtomicBool::new(false),
            #[cfg(test)]
            reconcile_started: std::sync::atomic::AtomicBool::new(false),
        }
    }

    pub(crate) fn from_env(directory: PathBuf) -> crate::Result<Self> {
        Ok(Self::new(directory, LogPolicy::from_env()?))
    }

    // A fresh descriptor per operation also serializes threads in one process.
    // Never unlink this file: doing so could create separate lock domains.
    fn lock(&self, nonblocking: bool) -> io::Result<File> {
        let path = self.directory.join(LOCK_NAME);
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if !metadata.file_type().is_file() => {
                return Err(io::Error::other(
                    "log coordination path is not a regular file",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)?;
        if nonblocking {
            file.try_lock().map_err(|error| match error {
                std::fs::TryLockError::WouldBlock => io::Error::from(io::ErrorKind::WouldBlock),
                std::fs::TryLockError::Error(error) => error,
            })?;
        } else {
            file.lock()?;
        }
        Ok(file)
    }

    fn inventory(&self) -> io::Result<Vec<LogFile>> {
        let mut files = Vec::new();
        for entry in std::fs::read_dir(&self.directory)? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let name = entry.file_name();
            let Some((date, index)) = name.to_str().and_then(parse_name) else {
                continue;
            };
            files.push(LogFile {
                path: entry.path(),
                date,
                index,
                bytes: entry.metadata()?.len(),
            });
        }
        files.sort_by_key(|file| (file.date, file.index));
        Ok(files)
    }

    fn preserve_index_floor(lock: &mut File, highest_seen: u64) -> io::Result<u64> {
        let recorded = match lock.metadata()?.len() {
            0 => 0,
            16 => {
                let mut bytes = [0; 16];
                lock.rewind()?;
                lock.read_exact(&mut bytes)?;
                let (first, second) = bytes.split_at(8);
                let value = u64::from_le_bytes(first.try_into().map_err(io::Error::other)?);
                let complement = u64::from_le_bytes(second.try_into().map_err(io::Error::other)?);
                if value != !complement {
                    return Err(io::Error::other("log rotation counter is corrupt"));
                }
                value
            }
            _ => {
                return Err(io::Error::other(
                    "log rotation counter has an invalid length",
                ));
            }
        };
        let newest = highest_seen.max(recorded);
        if newest > recorded {
            Self::write_index(lock, newest)?;
        }
        Ok(newest)
    }

    fn reserve_index(lock: &mut File, highest_seen: u64) -> io::Result<u64> {
        let newest = Self::preserve_index_floor(lock, highest_seen)?;
        let next = newest
            .checked_add(1)
            .ok_or_else(|| io::Error::other("log rotation sequence exhausted"))?;
        // Reserve before creation. Deleted names are never reused after a restart
        // or clock rollback, so a scanned archive path cannot name a new segment.
        Self::write_index(lock, next)?;
        Ok(next)
    }

    fn write_index(lock: &mut File, value: u64) -> io::Result<()> {
        lock.rewind()?;
        lock.write_all(&value.to_le_bytes())?;
        lock.write_all(&(!value).to_le_bytes())?;
        lock.sync_data()
    }

    fn prune(
        &self,
        files: &mut Vec<LogFile>,
        now: DateTime<Utc>,
        protected: Option<&Path>,
        limit: usize,
    ) -> io::Result<()> {
        let cutoff = now - chrono::Duration::days(RETENTION_DAYS);
        let mut index = 0;
        while index < files.len() {
            let file = &files[index];
            let expired = file.date < cutoff.date_naive()
                || (file.date == cutoff.date_naive() && cutoff.time() > chrono::NaiveTime::MIN);
            if protected != Some(file.path.as_path())
                && (expired || file.bytes > self.policy.max_file_bytes || files.len() > limit)
            {
                // Every cooperating writer closes its data handle before releasing
                // the lock, so none can be appending to a candidate we remove.
                #[cfg(test)]
                if self.fail_removal.load(Ordering::Relaxed) {
                    return Err(io::Error::from(io::ErrorKind::PermissionDenied));
                }
                std::fs::remove_file(&file.path)?;
                files.remove(index);
            } else {
                index += 1;
            }
        }
        if files.len() > limit {
            return Err(io::Error::other("unable to reclaim log file capacity"));
        }
        Ok(())
    }

    pub(super) fn reconcile(&self, now: DateTime<Utc>) -> io::Result<()> {
        #[cfg(test)]
        self.reconcile_started.store(true, Ordering::Release);
        let mut lock = self.lock(false)?;
        let mut files = self.inventory()?;
        Self::preserve_index_floor(
            &mut lock,
            files.iter().map(|file| file.index).max().unwrap_or(0),
        )?;
        // No data handle survives a write transaction; retention never truncates
        // a file or races an active append from a cooperating process.
        self.prune(&mut files, now, None, self.policy.max_files)
    }

    pub(super) fn initialize(&self) -> io::Result<()> {
        self.append(&[], Utc::now(), false)
    }

    fn append(&self, bytes: &[u8], now: DateTime<Utc>, nonblocking: bool) -> io::Result<()> {
        let mut lock = self.lock(nonblocking)?;
        let mut files = self.inventory()?;
        let highest_seen = files.iter().map(|file| file.index).max().unwrap_or(0);
        Self::preserve_index_floor(&mut lock, highest_seen)?;
        let record = if bytes.len() as u64 > self.policy.max_file_bytes {
            let count = self.truncated_records.fetch_add(1, Ordering::Relaxed) + 1;
            if count.is_power_of_two() {
                eprintln!("Log records truncated by byte limit: {count}");
            }
            let budget = self.policy.max_file_bytes as usize - TRUNCATED.len();
            let prefix = match std::str::from_utf8(&bytes[..budget]) {
                Ok(_) => budget,
                Err(error) => error.valid_up_to(),
            };
            let mut record = Vec::with_capacity(prefix + TRUNCATED.len());
            record.extend_from_slice(&bytes[..prefix]);
            record.extend_from_slice(TRUNCATED);
            std::borrow::Cow::Owned(record)
        } else {
            std::borrow::Cow::Borrowed(bytes)
        };
        let date = now.date_naive();
        let current = files.iter().rev().find(|file| file.date == date);
        let append_path = current
            .filter(|file| file.bytes <= self.policy.max_file_bytes - record.len() as u64)
            .map(|file| file.path.clone());
        let limit = if append_path.is_some() {
            self.policy.max_files
        } else {
            self.policy.max_files - 1
        };
        self.prune(&mut files, now, append_path.as_deref(), limit)?;
        let mut file = if let Some(path) = append_path {
            // The shared lock prevents replacement by cooperating writers; reject
            // preexisting links rather than writing through them.
            if !std::fs::symlink_metadata(&path)?.file_type().is_file() {
                return Err(io::Error::other("active log path is not a regular file"));
            }
            OpenOptions::new().append(true).open(path)?
        } else {
            loop {
                let index = Self::reserve_index(&mut lock, highest_seen)?;
                let path = self.directory.join(filename(date, index));
                match OpenOptions::new().create_new(true).write(true).open(path) {
                    Ok(file) => break file,
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(error),
                }
            }
        };
        file.write_all(&record)?;
        file.flush()
    }

    /// Panic-path ownership must never wait behind the logging worker or another
    /// process. The panic hook falls back to stderr if this attempt cannot write.
    pub(crate) fn try_append_emergency(&self, record: &str) -> io::Result<()> {
        self.append(record.as_bytes(), Utc::now(), true)
    }
}

pub(super) struct LogWriter(pub(super) Arc<LogStore>);

impl Write for LogWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if let Err(error) = self.0.append(bytes, Utc::now(), false) {
            let count = self.0.failed_writes.fetch_add(1, Ordering::Relaxed) + 1;
            if count.is_power_of_two() {
                eprintln!("File log writes failed: {count}; {error}");
            }
            return Err(error);
        }
        // Oversized records deliberately consume the original input after writing
        // a bounded prefix/marker, rather than retrying its discarded tail.
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
