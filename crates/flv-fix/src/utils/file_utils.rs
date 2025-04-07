use std::{
    fs,
    io::{self, Read, Seek, SeekFrom, Write},
    path::PathBuf,
};
use byteorder::{BigEndian, WriteBytesExt};
use flv::{
    tag::{FlvTagType},
    writer::FlvWriter,
};
use tracing::debug;

// Common constants used across the crate
pub const FLV_HEADER_SIZE: usize = 9;
pub const FLV_PREVIOUS_TAG_SIZE: usize = 4;
pub const FLV_TAG_HEADER_SIZE: usize = 11;
pub const DEFAULT_BUFFER_SIZE: usize = 64 * 1024; // 64KB chunks

/// Helper function to shift file content forward (when inserting larger data)
pub fn shift_content_forward<T: Read + Write + Seek>(
    file_handle: &mut T,
    next_tag_pos: u64, 
    total_file_size: u64,
    size_diff: i64
) -> io::Result<()> {
    debug!("Shifting content forward by {} bytes", size_diff);
    
    let mut chunk_buffer = vec![0u8; DEFAULT_BUFFER_SIZE];
    
    // Start from the end of the file and work backward
    let mut read_pos = total_file_size - DEFAULT_BUFFER_SIZE as u64;
    let mut write_pos = total_file_size + size_diff as u64;
    
    while read_pos >= next_tag_pos {
        // Adjust the read position to not go before the next tag position
        let actual_read_pos = read_pos.max(next_tag_pos);
        let bytes_to_read = (read_pos + DEFAULT_BUFFER_SIZE as u64 - actual_read_pos) as usize;
        
        if bytes_to_read == 0 {
            break;
        }
        
        // Read chunk from the original position
        file_handle.seek(SeekFrom::Start(actual_read_pos))?;
        let actual_read = file_handle.read(&mut chunk_buffer[0..bytes_to_read])?;
        
        // Write chunk to the new position
        let write_start_pos = write_pos - bytes_to_read as u64;
        file_handle.seek(SeekFrom::Start(write_start_pos))?;
        file_handle.write_all(&chunk_buffer[0..actual_read])?;
        file_handle.flush()?;
        
        // Move positions backward
        if read_pos >= DEFAULT_BUFFER_SIZE as u64 {
            read_pos -= DEFAULT_BUFFER_SIZE as u64;
            write_pos -= DEFAULT_BUFFER_SIZE as u64;
        } else {
            break;
        }
    }
    
    Ok(())
}

/// Helper function to shift file content backward (when inserting smaller data)
pub fn shift_content_backward<T: Read + Write + Seek>(
    file_handle: &mut T,
    next_tag_pos: u64,
    new_next_tag_pos: u64,
    total_file_size: u64
) -> io::Result<()> {
    debug!("Shifting content backward");
    
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
        file_handle.flush()?;
        
        read_pos += actual_read as u64;
        write_pos += actual_read as u64;
    }
    
    Ok(())
}

/// Write an FLV tag header and data to a file
pub fn write_flv_tag<T: Write + Seek>(
    file_handle: &mut T,
    position: u64,
    tag_type: FlvTagType,
    data: &[u8],
    timestamp: u32
) -> io::Result<()> {
    file_handle.seek(SeekFrom::Start(position))?;
    
    // Write tag header
    let mut flv_writer = FlvWriter::new(file_handle)?;
    flv_writer.write_tag_header(tag_type, data.len() as u32, timestamp)?;
    
    // Write data
    let file = flv_writer.into_inner()?;
    file.write_all(data)?;
    
    // Write previous tag size
    let tag_size = data.len() as u32 + FLV_TAG_HEADER_SIZE as u32;
    file.write_u32::<BigEndian>(tag_size)?;
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