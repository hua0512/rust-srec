use bytes::Bytes;
use flv::{tag::FlvTagType, writer::FlvWriter};
use std::{
    fs,
    io::{self, Read, Seek, SeekFrom, Write},
    path::PathBuf,
};
use tracing::debug;

// Common constants used across the crate
pub const FLV_HEADER_SIZE: usize = 9;
pub const FLV_PREVIOUS_TAG_SIZE: usize = 4;
pub const DEFAULT_BUFFER_SIZE: usize = 64 * 1024; // 64KB chunks

/// Helper function to shift file content forward (when inserting larger data)
pub fn shift_content_forward<T: Read + Write + Seek>(
    file_handle: &mut T,
    next_tag_pos: u64,    // Start of data to move
    total_file_size: u64, // Original end of data to move
    size_diff: i64,       // Positive number, how much space to add
) -> io::Result<()> {
    debug!("Shifting content forward by {} bytes", size_diff);
    debug_assert!(
        size_diff > 0,
        "shift_content_forward requires positive size_diff"
    );

    let mut chunk_buffer = vec![0u8; DEFAULT_BUFFER_SIZE];

    // Start from the end of the file and work backward
    let mut current_original_pos = total_file_size; // Position *after* the last byte to read
    let mut current_new_pos = total_file_size + size_diff as u64; // Position *after* the last byte to write

    // Loop while there's still data at or after next_tag_pos to be copied
    while current_original_pos > next_tag_pos {
        // How much data can we process in this chunk?
        // It's the minimum of the buffer size and the remaining data to copy.
        let amount_to_process =
            (current_original_pos - next_tag_pos).min(DEFAULT_BUFFER_SIZE as u64);

        if amount_to_process == 0 {
            break; // Should not happen if loop condition is correct, but safe check
        }

        // Calculate the read start position for this chunk
        let read_start_pos = current_original_pos - amount_to_process;

        // Calculate the write start position for this chunk
        let write_start_pos = current_new_pos - amount_to_process;

        // Read chunk from the original position
        file_handle.seek(SeekFrom::Start(read_start_pos))?;
        let chunk = &mut chunk_buffer[0..amount_to_process as usize];
        file_handle.read_exact(chunk)?;

        // Write chunk to the new position
        file_handle.seek(SeekFrom::Start(write_start_pos))?;
        file_handle.write_all(chunk)?;

        // Move positions backward for the next iteration
        current_original_pos = read_start_pos;
        current_new_pos = write_start_pos;
    }
    file_handle.flush()?; // Flush once at the end
    Ok(())
}

/// Helper function to shift file content backward (when inserting smaller data)
pub fn shift_content_backward<T: Read + Write + Seek>(
    file_handle: &mut T,
    next_tag_pos: u64,
    new_next_tag_pos: u64,
    total_file_size: u64,
) -> io::Result<()> {
    debug!("Shifting content backward");
    debug_assert!(
        new_next_tag_pos <= next_tag_pos,
        "shift_content_backward expects a non-increasing destination"
    );

    let mut chunk_buffer = vec![0u8; DEFAULT_BUFFER_SIZE];
    let mut read_pos = next_tag_pos;
    let mut write_pos = new_next_tag_pos;

    while read_pos < total_file_size {
        file_handle.seek(SeekFrom::Start(read_pos))?;

        let bytes_to_read = DEFAULT_BUFFER_SIZE.min((total_file_size - read_pos) as usize);
        let actual_read = file_handle.read(&mut chunk_buffer[0..bytes_to_read])?;

        if actual_read == 0 {
            break;
        }

        file_handle.seek(SeekFrom::Start(write_pos))?;
        file_handle.write_all(&chunk_buffer[0..actual_read])?;

        read_pos += actual_read as u64;
        write_pos += actual_read as u64;
    }

    file_handle.flush()?;
    Ok(())
}

/// Write an FLV tag header and data to a file
pub fn write_flv_tag<T: Write + Seek>(
    file_handle: &mut T,
    position: u64,
    tag_type: FlvTagType,
    data: &[u8],
    timestamp: u32,
) -> io::Result<()> {
    file_handle.seek(SeekFrom::Start(position))?;

    // Write a full FLV tag (header + payload + PreviousTagSize)
    let mut flv_writer = FlvWriter::new(file_handle)?;
    flv_writer.write_tag(tag_type, Bytes::copy_from_slice(data), timestamp)?;
    let file = flv_writer.into_inner()?;
    file.flush()?;

    Ok(())
}

/// Create a backup of a file
pub fn create_backup(file_path: &PathBuf) -> io::Result<PathBuf> {
    let backup_path = file_path.with_extension("flv.bak");
    fs::copy(file_path, &backup_path)?;
    debug!("Created backup at {}", backup_path.display());
    Ok(backup_path)
}
