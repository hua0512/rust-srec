//! # FLV Audio Module
//!
//! Implementation of FLV audio tag data parsing following the E-RTMP v2 specification.
//!
//! This module handles parsing of audio data from FLV (Flash Video) files, with support
//! for both legacy and enhanced audio formats.
//!
//! ## Specifications
//!
//! - [E-RTMP v2 specification](https://github.com/veovera/enhanced-rtmp/blob/main/docs/enhanced/enhanced-rtmp-v2.md#enhanced-audio)
//!
//! ## Credits
//!
//! Based on the work of [ScuffleCloud project](https://github.com/ScuffleCloud/scuffle/blob/main/crates/flv/src/audio.rs)
//!
//! ## License
//!
//! MIT License
//!
//! ## Authors
//!
//! - ScuffleCloud project contributors
//! - hua0512

use std::{fmt, io};

use byteorder::{BigEndian, ReadBytesExt};
use bytes::Bytes;
use bytes_util::BytesCursorExt;

use super::aac::{AacPacket, AacPacketType};

#[repr(u8)]
#[derive(Debug, Clone, PartialEq, Copy)]
pub enum SoundFormat {
    /// Uncompressed PCM audio
    Pcm = 0,
    /// ADPCM compressed audio
    AdPcm = 1,
    Mp3 = 2,
    PcmLe = 3,
    Nellymoser16khzMono = 4,
    Nellymoser8khzMono = 5,
    Nellymoser = 6,
    G711ALaw = 7,
    G711MuLaw = 8,
    // New in E-RTMP v2
    ExHeader = 9,
    Aac = 10,
    Speex = 11,
    Mp38k = 14,
    DeviceSpecific = 15,
}

impl TryFrom<u8> for SoundFormat {
    type Error = io::Error;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Ok(match value {
            0 => SoundFormat::Pcm,
            1 => SoundFormat::AdPcm,
            2 => SoundFormat::Mp3,
            3 => SoundFormat::PcmLe,
            4 => SoundFormat::Nellymoser16khzMono,
            5 => SoundFormat::Nellymoser8khzMono,
            6 => SoundFormat::Nellymoser,
            7 => SoundFormat::G711ALaw,
            8 => SoundFormat::G711MuLaw,
            9 => SoundFormat::ExHeader,
            10 => SoundFormat::Aac,
            11 => SoundFormat::Speex,
            14 => SoundFormat::Mp38k,
            15 => SoundFormat::DeviceSpecific,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Invalid sound format: {}", value),
                ));
            }
        })
    }
}

#[repr(u8)]
#[derive(Debug, Clone, PartialEq, Copy)]
pub enum SoundRate {
    Hz5512 = 0,
    Hz11025 = 1,
    Hz22050 = 2,
    Hz44100 = 3,
    // New in E-RTMP v2
    Hz48000 = 4,
}

impl TryFrom<u8> for SoundRate {
    type Error = io::Error;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Ok(match value {
            0 => SoundRate::Hz5512,
            1 => SoundRate::Hz11025,
            2 => SoundRate::Hz22050,
            3 => SoundRate::Hz44100,
            4 => SoundRate::Hz48000,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Invalid sound rate: {}", value),
                ));
            }
        })
    }
}

// Representation of sound size in Audio Data in FLV
#[derive(Debug, Clone, PartialEq, Copy)]
pub enum SoundSize {
    Bits8 = 0,
    Bits16 = 1,
    // New in E-RTMP v2
    Bits24 = 2,
}
impl TryFrom<u8> for SoundSize {
    type Error = io::Error;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Ok(match value {
            0 => SoundSize::Bits8,
            1 => SoundSize::Bits16,
            2 => SoundSize::Bits24,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Invalid sound size: {}", value),
                ));
            }
        })
    }
}

#[repr(u8)]
#[derive(Debug, Clone, PartialEq, Copy)]
pub enum SoundType {
    Mono = 0,
    Stereo = 1,
}

impl TryFrom<u8> for SoundType {
    type Error = io::Error;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Ok(match value {
            0 => SoundType::Mono,
            1 => SoundType::Stereo,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Invalid sound type: {}", value),
                ));
            }
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AudioLegacyPacket {
    // bit 3-2
    pub sound_rate: SoundRate,
    // bit 1
    pub sound_size: SoundSize,
    // bit 0
    pub sound_type: SoundType,
}

impl AudioLegacyPacket {
    fn new(sound_rate: SoundRate, sound_size: SoundSize, sound_type: SoundType) -> Self {
        AudioLegacyPacket {
            sound_rate,
            sound_size,
            sound_type,
        }
    }

    pub fn from_byte(byte: u8) -> Result<Self, io::Error> {
        // Extract bits 3-2 for sound rate
        const SOUND_RATE_MASK: u8 = 0b00001100;
        const SOUND_RATE_SHIFT: u8 = 2;
        let sound_rate = SoundRate::try_from((byte & SOUND_RATE_MASK) >> SOUND_RATE_SHIFT)?;
        // Extract bit 1 for sound size
        const SOUND_SIZE_MASK: u8 = 0b00000010;
        const SOUND_SIZE_SHIFT: u8 = 1;
        let sound_size = SoundSize::try_from((byte & SOUND_SIZE_MASK) >> SOUND_SIZE_SHIFT)?;
        // Extract bit 0 for sound type
        const SOUND_TYPE_MASK: u8 = 0b00000001;
        let sound_type = SoundType::try_from(byte & SOUND_TYPE_MASK)?;

        Ok(AudioLegacyPacket::new(sound_rate, sound_size, sound_type))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AudioHeader {
    // sound format
    pub sound_format: SoundFormat,
    pub packet: AudioPacket,
}

// Representation of audio data in FLV
#[derive(Debug, Clone, PartialEq)]
pub struct AudioData {
    pub header: AudioHeader,
    // Body
    pub body: AudioDataBody,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AudioDataBody {
    // Usually the body of an audio tag is a aac packet
    Aac(AacPacket),
    /// Some other audio format we don't know how to parse
    Unknown {
        data: Bytes,
    },
}

impl AudioDataBody {
    pub fn demux(
        sound_format: &SoundFormat,
        reader: &mut io::Cursor<Bytes>,
        body_size: Option<usize>,
    ) -> io::Result<Self> {
        match sound_format {
            SoundFormat::Aac => {
                // For some reason the spec adds a specific byte before the AAC data.
                // This byte is the AAC packet type.
                let aac_packet_type = AacPacketType::try_from(reader.read_u8()?)?;
                Ok(Self::Aac(AacPacket::new(
                    aac_packet_type,
                    AudioData::read_remaining(reader, body_size)?,
                )))
            }
            _ => Ok(Self::Unknown {
                data: AudioData::read_remaining(reader, body_size)?,
            }),
        }
    }

    /// Check if the audio data is a sequence header
    pub fn is_sequence_header(&self) -> bool {
        match self {
            AudioDataBody::Aac(packet) => packet.is_sequence_header(),
            _ => false,
        }
    }

    pub fn is_stereo(&self) -> bool {
        match self {
            AudioDataBody::Aac(packet) => packet.is_stereo(),

            _ => false,
        }
    }

    pub fn sample_rate(&self) -> f32 {
        match self {
            AudioDataBody::Aac(packet) => packet.sample_rate(),
            _ => 0.0,
        }
    }

    pub fn sample_size(&self) -> u32 {
        match self {
            AudioDataBody::Aac(packet) => packet.sample_size(),
            AudioDataBody::Unknown { .. } => 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum AudioPacket {
    // Legacy audio packet, this is the default
    Legacy(AudioLegacyPacket),
    // New in E-RTMP v2, Audio Packet Type
    AudioPacketType(AudioPacketType),
}

// New in E-RTMP v2, Audio Packet Type
#[repr(u8)]
#[derive(Debug, Clone, PartialEq, Copy)]
pub enum AudioPacketType {
    SequenceStart = 0,
    CodecFrames = 1,
    SequenceEnd = 2,
    MultichannelConfig = 4,
    Multitrack = 5,
    ModEx = 7,
}

impl TryFrom<u8> for AudioPacketType {
    type Error = io::Error;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Ok(match value {
            0 => AudioPacketType::SequenceStart,
            1 => AudioPacketType::CodecFrames,
            2 => AudioPacketType::SequenceEnd,
            4 => AudioPacketType::MultichannelConfig,
            5 => AudioPacketType::Multitrack,
            7 => AudioPacketType::ModEx,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Invalid audio packet type: {}", value),
                ));
            }
        })
    }
}

#[derive(Debug, Clone, PartialEq, Copy)]
pub enum AudioPacketModExType {
    TimestampOffsetNano = 0,
}

impl TryFrom<u8> for AudioPacketModExType {
    type Error = io::Error;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Ok(match value {
            0 => AudioPacketModExType::TimestampOffsetNano,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Invalid audio packet modex type: {}", value),
                ));
            }
        })
    }
}

#[repr(u8)]
#[derive(Debug, Clone, PartialEq, Copy)]
// New in E-RTMP v2, Audio FourCC
pub enum AudioFourCC {
    Ac3,
    Eac3,
    Opus,
    Mp3,
    Flac,
    Aac,
}

impl AudioFourCC {
    pub fn from_u32(value: u32) -> Result<Self, io::Error> {
        Ok(match value {
            // Note: The spec uses ASCII representations, but maps them to these u32 values
            // in the binary stream. We'll use the numeric values for matching.
            // These values seem arbitrary in the spec draft, double check if finalized.
            // Let's assume the spec means these literal u32 values for now.
            // If it meant the ASCII codes as u32, e.g., 'Opus' -> 0x4f707573, adjust accordingly.
            // The current spec text is a bit ambiguous here. Assuming numeric mapping:
            0x61632D33 => AudioFourCC::Ac3,  // "ac-3"
            0x6561632D => AudioFourCC::Eac3, // "eac-" (assuming eac-3)
            0x4F707573 => AudioFourCC::Opus, // "Opus"
            0x2E6D7033 => AudioFourCC::Mp3,  // ".mp3"
            0x664C6143 => AudioFourCC::Flac, // "fLaC"
            0x6D703461 => AudioFourCC::Aac,  // "mp4a" (Common FourCC for AAC)
            // Alternative AAC FourCC if needed: 0x61616300 => AudioFourCC::Aac, // "aac\0"
            _ => {
                // Try matching common ASCII representations as well
                match value {
                    0x65616333 => AudioFourCC::Eac3, // "eac3"
                    0x61616300 => AudioFourCC::Aac,  // "aac\0" (as used in as_bytes)
                    _ => {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("Invalid or unknown audio fourcc: 0x{:08x}", value),
                        ));
                    }
                }
            }
        })
    }

    pub fn as_bytes(&self) -> &'static [u8] {
        match self {
            AudioFourCC::Ac3 => b"ac-3", // Use consistent representation
            AudioFourCC::Eac3 => b"eac3",
            AudioFourCC::Opus => b"Opus",
            AudioFourCC::Mp3 => b".mp3",
            AudioFourCC::Flac => b"fLaC", // Match spec example
            AudioFourCC::Aac => b"mp4a",  // Common FourCC
        }
    }
}

#[repr(u8)]
#[derive(Debug, Clone, PartialEq, Copy)]
pub enum AvMultitrackType {
    OneTrack = 0,
    ManyTracks = 1,
    ManyTracksManyCodecs = 2,
}

impl TryFrom<u8> for AvMultitrackType {
    type Error = io::Error;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Ok(match value {
            0 => AvMultitrackType::OneTrack,
            1 => AvMultitrackType::ManyTracks,
            2 => AvMultitrackType::ManyTracksManyCodecs,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Invalid av multitrack type: {}", value),
                ));
            }
        })
    }
}

impl AudioData {
    /// Parses audio data from an FLV tag
    ///
    /// # Arguments
    ///
    /// * `reader` - A cursor positioned at the start of the audio data
    /// * `body_size` - The expected size of the audio data in bytes, or None to read all available data
    ///
    /// # Returns
    ///
    /// An `AudioData` structure containing the parsed header and body data
    ///
    /// # Errors
    ///
    /// Returns an `io::Error` if:
    /// * Reading from the cursor fails
    /// * The data format is invalid or unsupported
    pub fn demux(reader: &mut io::Cursor<Bytes>, body_size: Option<usize>) -> io::Result<Self> {
        let start_pos = reader.position() as usize;

        // Read the first byte to get the sound format
        let sound_format_byte = reader.read_u8()?;

        let available_data = reader.get_ref().len() - start_pos as usize;

        // Ensure we have at least the first byte
        if available_data < 1 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "No data available for audio header byte",
            ));
        }

        // Parse the sound format (bits 7-4)
        let sound_format = SoundFormat::try_from(sound_format_byte >> 4)?;

        let audio_packet = match sound_format {
            // New in E-RTMP v2, new header
            SoundFormat::ExHeader => {
                // Switch to the new fourcc mode
                let audio_packet_type = AudioPacketType::try_from(sound_format_byte & 0x0F)
                    .map_err(|_| {
                        io::Error::new(io::ErrorKind::InvalidData, "Invalid audio packet type")
                    })?;

                match audio_packet_type {
                    AudioPacketType::ModEx => {
                        // Process ModEx packets in a loop until we get a non-ModEx packet type
                        let mut final_audio_packet_type = audio_packet_type;
                        let mut audio_timestamp_nano_offset = 0u32;

                        // Determine the size of the packet ModEx data (ranging from 1 to 256 bytes)
                        let mod_ex_data_size = (reader.read_u8()? as usize) + 1;

                        // If maximum 8-bit size is not sufficient, use a 16-bit value
                        let mod_ex_data_size = if mod_ex_data_size == 256 {
                            (reader.read_u16::<BigEndian>()? as usize) + 1
                        } else {
                            mod_ex_data_size
                        };

                        // Fetch the packet ModEx data based on its determined size
                        let mod_ex_data = reader.extract_bytes(mod_ex_data_size)?;

                        // Check the length of mod_ex_data once before entering the loop
                        if mod_ex_data.len() >= 3 {
                            audio_timestamp_nano_offset = ((mod_ex_data[0] as u32) << 16)
                                | ((mod_ex_data[1] as u32) << 8)
                                | (mod_ex_data[2] as u32);
                            // Note: The audio_timestamp_nano_offset could be stored in the AudioData struct
                            // and used for precise timing calculations
                        }

                        loop {
                            // Fetch the AudioPacketModExType
                            let next_byte = reader.read_u8()?;
                            let audio_packet_mod_ex_type =
                                AudioPacketModExType::try_from(next_byte >> 4)?;
                            let next_audio_packet_type =
                                AudioPacketType::try_from(next_byte & 0x0F)?;

                            // Break the loop if the next packet type is not ModEx
                            if next_audio_packet_type != AudioPacketType::ModEx {
                                final_audio_packet_type = next_audio_packet_type;
                                break;
                            }
                        }

                        // Continue with the final non-ModEx audio packet type
                        let mut is_audio_multitrack = false;
                        let mut audio_multitrack_type = AvMultitrackType::OneTrack;
                        let mut audio_four_cc = None;

                        if final_audio_packet_type == AudioPacketType::Multitrack {
                            is_audio_multitrack = true;

                            // Read the multitrack type
                            let multitrack_type_byte = reader.read_u8()?;
                            audio_multitrack_type =
                                AvMultitrackType::try_from(multitrack_type_byte & 0x0F)?;

                            // Fetch AudioPacketType for all audio tracks in the audio message
                            let packet_type_byte = reader.read_u8()?;
                            let new_packet_type =
                                AudioPacketType::try_from(packet_type_byte & 0x0F)?;

                            // Make sure it's not multitrack again
                            if new_packet_type == AudioPacketType::Multitrack {
                                return Err(io::Error::new(
                                    io::ErrorKind::InvalidData,
                                    "Nested multitrack is not allowed",
                                ));
                            }

                            final_audio_packet_type = new_packet_type;

                            if audio_multitrack_type != AvMultitrackType::ManyTracksManyCodecs {
                                // The tracks are encoded with the same codec, read the FOURCC for them
                                let four_cc = reader.read_u32::<BigEndian>()?;
                                audio_four_cc = match AudioFourCC::from_u32(four_cc) {
                                    Ok(four_cc) => Some(four_cc),
                                    Err(e) => {
                                        return Err(io::Error::new(
                                            io::ErrorKind::InvalidData,
                                            format!("Invalid FOURCC value: {}", e),
                                        ));
                                    }
                                };
                            }
                        } else {
                            // Not multitrack, read the FOURCC
                            let four_cc = reader.read_u32::<BigEndian>()?;
                            audio_four_cc = Some(AudioFourCC::from_u32(four_cc)?);
                        }

                        // Now we're ready to process the audio body based on final_audio_packet_type
                        // For now, we'll just return a placeholder

                        AudioPacket::AudioPacketType(final_audio_packet_type)
                    }
                    // Other audio packet types, not implemented yet
                    other => AudioPacket::AudioPacketType(other),
                }
            }
            // Legacy audio packet
            _ => AudioPacket::Legacy(AudioLegacyPacket::from_byte(sound_format_byte)?),
        };

        // read the body
        let original_position = reader.position();
        let body = match AudioDataBody::demux(&sound_format, reader, body_size) {
            Ok(body) => body,
            Err(e) => {
                reader.set_position(original_position); // Reset reader position on error
                return Err(e);
            }
        };

        // flv audio header
        let audio_header = AudioHeader {
            sound_format: sound_format,
            packet: audio_packet,
        };

        Ok(AudioData {
            header: audio_header,
            body,
        })
    }

    fn read_remaining(reader: &mut io::Cursor<Bytes>, size: Option<usize>) -> io::Result<Bytes> {
        Ok(if size == None || size.unwrap() == 0 {
            // if size is 0, read until the end of the stream
            reader.extract_remaining()
        } else {
            // read a fixed size
            reader.extract_bytes(size.unwrap())?
        })
    }
}

//-----------------------------------------------------------------------------
// Utility Struct for Quick Header Inspection
//-----------------------------------------------------------------------------

/// Provides efficient access to basic audio tag parameters by inspecting
/// the initial byte(s) without full parsing.
#[derive(Debug, Clone)]
pub struct AudioTagUtils {
    data: Bytes,
}

impl AudioTagUtils {
    /// Creates a new utility wrapper around the audio tag data.
    pub fn new(data: Bytes) -> Self {
        Self { data }
    }

    /// Gets the first byte containing format and legacy parameters.
    fn get_first_byte(&self) -> io::Result<u8> {
        self.data
            .first()
            .copied()
            .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "Audio tag data is empty"))
    }

    /// Gets the sound format (e.g., AAC, MP3, ExHeader).
    /// Reads only the first byte.
    pub fn sound_format(&self) -> io::Result<SoundFormat> {
        let byte = self.get_first_byte()?;
        SoundFormat::try_from(byte >> 4)
    }

    /// Checks if the format is a legacy FLV format (not ExHeader).
    /// Reads only the first byte.
    pub fn is_legacy(&self) -> io::Result<bool> {
        self.sound_format().map(|fmt| fmt != SoundFormat::ExHeader)
    }

    /// Checks if the format is an Enhanced RTMP header (ExHeader).
    /// Reads only the first byte.
    pub fn is_enhanced(&self) -> io::Result<bool> {
        self.sound_format().map(|fmt| fmt == SoundFormat::ExHeader)
    }

    /// Gets the sound rate (sample rate indicator) for *legacy* formats.
    /// Returns an error if the format is ExHeader.
    /// Reads only the first byte.
    pub fn sound_rate(&self) -> io::Result<SoundRate> {
        let byte = self.get_first_byte()?;
        if self.is_enhanced()? {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "SoundRate is not directly available in the first byte for ExHeader format",
            ));
        }
        const SOUND_RATE_MASK: u8 = 0b00001100;
        const SOUND_RATE_SHIFT: u8 = 2;
        SoundRate::try_from((byte & SOUND_RATE_MASK) >> SOUND_RATE_SHIFT)
    }

    /// Gets the sound size (bit depth indicator) for *legacy* formats.
    /// Returns an error if the format is ExHeader.
    /// Reads only the first byte.
    pub fn sound_size(&self) -> io::Result<SoundSize> {
        let byte = self.get_first_byte()?;
        if self.is_enhanced()? {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "SoundSize is not directly available in the first byte for ExHeader format",
            ));
        }
        const SOUND_SIZE_MASK: u8 = 0b00000010;
        const SOUND_SIZE_SHIFT: u8 = 1;
        SoundSize::try_from((byte & SOUND_SIZE_MASK) >> SOUND_SIZE_SHIFT)
    }

    /// Gets the sound type (Mono/Stereo indicator) for *legacy* formats.
    /// Returns an error if the format is ExHeader.
    /// Reads only the first byte.
    pub fn sound_type(&self) -> io::Result<SoundType> {
        let byte = self.get_first_byte()?;
        if self.is_enhanced()? {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "SoundType is not directly available in the first byte for ExHeader format",
            ));
        }
        const SOUND_TYPE_MASK: u8 = 0b00000001;
        SoundType::try_from(byte & SOUND_TYPE_MASK)
    }

    /// Gets the Audio Packet Type for *enhanced* (ExHeader) formats.
    /// Returns an error if the format is legacy.
    /// Reads only the first byte.
    /// Note: This might be AudioPacketType::ModEx or AudioPacketType::Multitrack,
    /// requiring further header parsing for the *actual* content packet type and FourCC.
    pub fn audio_packet_type(&self) -> io::Result<AudioPacketType> {
        let byte = self.get_first_byte()?;
        if self.is_legacy()? {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "AudioPacketType is only available for ExHeader format",
            ));
        }
        const PACKET_TYPE_MASK: u8 = 0x0F;
        AudioPacketType::try_from(byte & PACKET_TYPE_MASK)
    }

    // /// Attempts to get the FourCC for *enhanced* (ExHeader) formats.
    // /// This requires reading beyond the first byte and skipping potential ModEx/Multitrack headers.
    // /// Returns None if the format is legacy or if FourCC cannot be determined quickly.
    // /// WARNING: This is less efficient than other methods and might fail on complex headers.
    // pub fn try_get_four_cc(&self) -> io::Result<Option<AudioFourCC>> {
    //     if self.is_legacy()? {
    //         return Ok(None);
    //     }
    //     // Need to implement logic similar to the start of AudioData::demux
    //     // to skip ModEx/Multitrack and read the u32 FourCC.
    //     // This adds complexity and reads more bytes.
    //     // For simplicity, this is omitted here. Use full demux for reliable FourCC.
    //      Err(io::Error::new(io::ErrorKind::Unsupported, "Quick FourCC extraction not implemented yet, use full demux"))
    // }
}

impl fmt::Display for AudioData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.header, self.body)
    }
}

impl fmt::Display for AudioHeader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.sound_format, self.packet)
    }
}

impl fmt::Display for SoundFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SoundFormat::Pcm => write!(f, "PCM"),
            SoundFormat::AdPcm => write!(f, "ADPCM"),
            SoundFormat::Mp3 => write!(f, "MP3"),
            SoundFormat::PcmLe => write!(f, "PCM-LE"),
            SoundFormat::Nellymoser16khzMono => write!(f, "Nellymoser-16kHz-Mono"),
            SoundFormat::Nellymoser8khzMono => write!(f, "Nellymoser-8kHz-Mono"),
            SoundFormat::Nellymoser => write!(f, "Nellymoser"),
            SoundFormat::G711ALaw => write!(f, "G711-A-Law"),
            SoundFormat::G711MuLaw => write!(f, "G711-Mu-Law"),
            SoundFormat::ExHeader => write!(f, "Enhanced"),
            SoundFormat::Aac => write!(f, "AAC"),
            SoundFormat::Speex => write!(f, "Speex"),
            SoundFormat::Mp38k => write!(f, "MP3-8kHz"),
            SoundFormat::DeviceSpecific => write!(f, "DeviceSpecific"),
        }
    }
}

impl fmt::Display for AudioPacket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AudioPacket::Legacy(legacy) => write!(
                f,
                "Legacy[Rate: {}, Size: {}, Type: {}]",
                legacy.sound_rate, legacy.sound_size, legacy.sound_type
            ),
            AudioPacket::AudioPacketType(apt) => write!(f, "Enhanced[{}]", apt),
        }
    }
}

impl fmt::Display for AudioPacketType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AudioPacketType::SequenceStart => write!(f, "SequenceStart"),
            AudioPacketType::CodecFrames => write!(f, "CodecFrames"),
            AudioPacketType::SequenceEnd => write!(f, "SequenceEnd"),
            AudioPacketType::MultichannelConfig => write!(f, "MultichannelConfig"),
            AudioPacketType::Multitrack => write!(f, "Multitrack"),
            AudioPacketType::ModEx => write!(f, "ModEx"),
        }
    }
}

impl fmt::Display for AudioDataBody {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AudioDataBody::Aac(packet) => write!(f, "{}", packet),
            AudioDataBody::Unknown { data } => write!(f, "Unknown[{} bytes]", data.len()),
        }
    }
}

impl fmt::Display for SoundRate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SoundRate::Hz5512 => write!(f, "5.5kHz"),
            SoundRate::Hz11025 => write!(f, "11kHz"),
            SoundRate::Hz22050 => write!(f, "22kHz"),
            SoundRate::Hz44100 => write!(f, "44.1kHz"),
            SoundRate::Hz48000 => write!(f, "48kHz"),
        }
    }
}

impl fmt::Display for SoundSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SoundSize::Bits8 => write!(f, "8-bit"),
            SoundSize::Bits16 => write!(f, "16-bit"),
            SoundSize::Bits24 => write!(f, "24-bit"),
        }
    }
}

impl fmt::Display for SoundType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SoundType::Mono => write!(f, "Mono"),
            SoundType::Stereo => write!(f, "Stereo"),
        }
    }
}

impl fmt::Display for AudioFourCC {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AudioFourCC::Ac3 => write!(f, "AC-3"),
            AudioFourCC::Eac3 => write!(f, "E-AC-3"),
            AudioFourCC::Opus => write!(f, "Opus"),
            AudioFourCC::Mp3 => write!(f, "MP3"),
            AudioFourCC::Flac => write!(f, "FLAC"),
            AudioFourCC::Aac => write!(f, "AAC"),
        }
    }
}

impl fmt::Display for AvMultitrackType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AvMultitrackType::OneTrack => write!(f, "OneTrack"),
            AvMultitrackType::ManyTracks => write!(f, "ManyTracks"),
            AvMultitrackType::ManyTracksManyCodecs => write!(f, "ManyTracksManyCodecs"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use bytes::{BufMut, BytesMut};

    use super::*;

    #[test]
    fn test_parse_aac_audio_packet() {
        let mut reader = io::Cursor::new(Bytes::from(vec![0b10101101, 0b00000000, 1, 2, 3]));
        let audio_data = AudioData::demux(&mut reader, None).unwrap();

        assert_eq!(audio_data.header.sound_format, SoundFormat::Aac);
        match &audio_data.header.packet {
            AudioPacket::Legacy(legacy) => {
                assert_eq!(legacy.sound_rate, SoundRate::Hz44100);
                assert_eq!(legacy.sound_size, SoundSize::Bits8);
                assert_eq!(legacy.sound_type, SoundType::Stereo);
            }
            _ => panic!("Expected Legacy packet"),
        }
    }

    #[test]
    fn test_parse_unknown_audio_format() {
        let mut bytes = BytesMut::new();
        // MP3 sound format (2 << 4) with mono (0) and Hz11025 (2 << 2) and 16-bit samples (1 << 1)
        bytes.put_u8(0x26); // 0010 0110
        // Some MP3 data
        bytes.put_u8(0xAB);
        bytes.put_u8(0xCD);

        let mut cursor = Cursor::new(bytes.freeze());
        let audio_data = AudioData::demux(&mut cursor, None).unwrap();

        assert_eq!(audio_data.header.sound_format, SoundFormat::Mp3);
        match &audio_data.header.packet {
            AudioPacket::Legacy(legacy) => {
                assert_eq!(legacy.sound_rate, SoundRate::Hz11025);
                assert_eq!(legacy.sound_size, SoundSize::Bits16);
                assert_eq!(legacy.sound_type, SoundType::Mono);
            }
            _ => panic!("Expected Legacy packet"),
        }
        match &audio_data.body {
            AudioDataBody::Unknown { data } => {
                assert_eq!(data.len(), 2);
                assert_eq!(data[0], 0xAB);
                assert_eq!(data[1], 0xCD);
            }
            _ => panic!("Expected Unknown packet"),
        }
    }

    #[test]
    fn test_read_remaining_with_size() {
        let mut bytes = BytesMut::new();
        bytes.put_u8(0x01);
        bytes.put_u8(0x02);
        bytes.put_u8(0x03);
        bytes.put_u8(0x04);

        let mut cursor = Cursor::new(bytes.freeze());
        let result = AudioData::read_remaining(&mut cursor, Some(2)).unwrap();

        assert_eq!(result.len(), 2);
        assert_eq!(result[0], 0x01);
        assert_eq!(result[1], 0x02);
        assert_eq!(cursor.position(), 2);
    }

    #[test]
    fn test_read_remaining_without_size() {
        let mut bytes = BytesMut::new();
        bytes.put_u8(0x01);
        bytes.put_u8(0x02);
        bytes.put_u8(0x03);

        let mut cursor = Cursor::new(bytes.freeze());
        let result = AudioData::read_remaining(&mut cursor, None).unwrap();

        assert_eq!(result.len(), 3);
        assert_eq!(result[0], 0x01);
        assert_eq!(result[1], 0x02);
        assert_eq!(result[2], 0x03);
        assert_eq!(cursor.position(), 3);
    }
}
