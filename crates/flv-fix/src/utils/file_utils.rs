use std::{
    fs,
    io::{self, Seek, Write},
    path::PathBuf,
};

use bytes::Bytes;
use flv::{FlvWriter, tag::FlvTagType};
use tracing::debug;

pub const FLV_HEADER_SIZE: usize = 9;
pub const FLV_PREVIOUS_TAG_SIZE: usize = 4;
pub const DEFAULT_BUFFER_SIZE: usize = 64 * 1024;

/// Write an FLV tag header and data to a file.
pub fn write_flv_tag<T: Write + Seek>(
    file_handle: &mut T,
    position: u64,
    tag_type: FlvTagType,
    data: &[u8],
    timestamp: u32,
) -> io::Result<()> {
    file_handle.seek(io::SeekFrom::Start(position))?;

    let mut flv_writer = FlvWriter::new(file_handle)?;
    flv_writer.write_tag(tag_type, Bytes::copy_from_slice(data), timestamp)?;
    let file = flv_writer.into_inner()?;
    file.flush()?;

    Ok(())
}

/// Create a backup of an FLV file.
pub fn create_backup(file_path: &PathBuf) -> io::Result<PathBuf> {
    let backup_path = file_path.with_extension("flv.bak");
    fs::copy(file_path, &backup_path)?;
    debug!(path = %backup_path.display(), "Created FLV backup");
    Ok(backup_path)
}
