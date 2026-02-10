use bytes::Bytes;

use crate::{Result, TsError};

/// SCTE-35 table ID
pub const SCTE35_TABLE_ID: u8 = 0xFC;

/// SCTE-35 registration format identifier
pub const SCTE35_FORMAT_IDENTIFIER: [u8; 4] = *b"CUEI";

/// SCTE-35 splice command types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpliceCommandType {
    SpliceNull,
    SpliceSchedule,
    SpliceInsert,
    TimeSignal,
    BandwidthReservation,
    PrivateCommand,
    Unknown(u8),
}

impl From<u8> for SpliceCommandType {
    fn from(value: u8) -> Self {
        match value {
            0x00 => SpliceCommandType::SpliceNull,
            0x04 => SpliceCommandType::SpliceSchedule,
            0x05 => SpliceCommandType::SpliceInsert,
            0x06 => SpliceCommandType::TimeSignal,
            0x07 => SpliceCommandType::BandwidthReservation,
            0xFF => SpliceCommandType::PrivateCommand,
            v => SpliceCommandType::Unknown(v),
        }
    }
}

/// Parsed splice command
#[derive(Debug, Clone)]
pub enum SpliceCommand {
    SpliceNull,
    SpliceInsert(SpliceInsert),
    TimeSignal(TimeSignal),
    Other(Vec<u8>),
}

/// SCTE-35 splice insert command
#[derive(Debug, Clone)]
pub struct SpliceInsert {
    pub splice_event_id: u32,
    pub splice_event_cancel_indicator: bool,
    pub out_of_network_indicator: bool,
    pub program_splice_flag: bool,
    pub splice_immediate_flag: bool,
    pub splice_time: Option<u64>,
    pub duration: Option<BreakDuration>,
    pub unique_program_id: u16,
    pub avail_num: u8,
    pub avails_expected: u8,
}

/// Break duration in a splice insert
#[derive(Debug, Clone, Copy)]
pub struct BreakDuration {
    pub auto_return: bool,
    /// Duration in 90kHz ticks (33-bit)
    pub duration: u64,
}

/// SCTE-35 time signal command
#[derive(Debug, Clone)]
pub struct TimeSignal {
    pub splice_time: Option<u64>,
}

/// Parse a splice_time() structure. Returns (time_value, bytes_consumed).
fn parse_splice_time(data: &[u8]) -> (Option<u64>, usize) {
    if data.is_empty() {
        return (None, 0);
    }
    let time_specified_flag = (data[0] & 0x80) != 0;
    if time_specified_flag {
        if data.len() < 5 {
            return (None, data.len());
        }
        let pts = (((data[0] as u64) & 0x01) << 32)
            | ((data[1] as u64) << 24)
            | ((data[2] as u64) << 16)
            | ((data[3] as u64) << 8)
            | (data[4] as u64);
        (Some(pts), 5)
    } else {
        (None, 1)
    }
}

/// Parse a break_duration() structure
fn parse_break_duration(data: &[u8]) -> Option<BreakDuration> {
    if data.len() < 5 {
        return None;
    }
    let auto_return = (data[0] & 0x80) != 0;
    let duration = (((data[0] as u64) & 0x01) << 32)
        | ((data[1] as u64) << 24)
        | ((data[2] as u64) << 16)
        | ((data[3] as u64) << 8)
        | (data[4] as u64);
    Some(BreakDuration {
        auto_return,
        duration,
    })
}

/// Top-level SCTE-35 splice info section (owned).
#[derive(Debug, Clone)]
pub struct SpliceInfoSection {
    pub table_id: u8,
    pub protocol_version: u8,
    pub encrypted_packet: bool,
    pub pts_adjustment: u64,
    pub splice_command_type: SpliceCommandType,
    pub splice_command: SpliceCommand,
}

impl SpliceInfoSection {
    /// Parse a SCTE-35 splice info section from PSI section data.
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < 14 {
            return Err(TsError::InsufficientData {
                expected: 14,
                actual: data.len(),
            });
        }

        let table_id = data[0];
        if table_id != SCTE35_TABLE_ID {
            return Err(TsError::InvalidTableId {
                expected: SCTE35_TABLE_ID,
                actual: table_id,
            });
        }

        let _section_length = ((data[1] as u16 & 0x0F) << 8) | data[2] as u16;
        let protocol_version = data[3];
        let encrypted_packet = (data[4] & 0x80) != 0;

        // pts_adjustment is 33 bits starting at bit 6 of byte 4
        let pts_adjustment = (((data[4] as u64) & 0x01) << 32)
            | ((data[5] as u64) << 24)
            | ((data[6] as u64) << 16)
            | ((data[7] as u64) << 8)
            | (data[8] as u64);

        // cw_index at byte 9
        // tier at bytes 10-11 (12 bits)
        let splice_command_length = ((data[11] as u16 & 0x0F) << 8) | data[12] as u16;
        let splice_command_type = SpliceCommandType::from(data[13]);

        let cmd_start = 14;
        let cmd_end = if splice_command_length == 0xFFF {
            // Unknown length — use rest of section minus CRC
            data.len().saturating_sub(4)
        } else {
            (cmd_start + splice_command_length as usize).min(data.len())
        };

        let cmd_data = if cmd_start < cmd_end {
            &data[cmd_start..cmd_end]
        } else {
            &[]
        };

        let splice_command = match splice_command_type {
            SpliceCommandType::SpliceNull => SpliceCommand::SpliceNull,
            SpliceCommandType::SpliceInsert => match Self::parse_splice_insert(cmd_data) {
                Ok(insert) => SpliceCommand::SpliceInsert(insert),
                Err(_) => SpliceCommand::Other(cmd_data.to_vec()),
            },
            SpliceCommandType::TimeSignal => {
                let (splice_time, _) = parse_splice_time(cmd_data);
                SpliceCommand::TimeSignal(TimeSignal { splice_time })
            }
            _ => SpliceCommand::Other(cmd_data.to_vec()),
        };

        Ok(SpliceInfoSection {
            table_id,
            protocol_version,
            encrypted_packet,
            pts_adjustment,
            splice_command_type,
            splice_command,
        })
    }

    fn parse_splice_insert(data: &[u8]) -> Result<SpliceInsert> {
        if data.len() < 5 {
            return Err(TsError::InvalidScte35(
                "splice_insert too short".to_string(),
            ));
        }

        let splice_event_id = ((data[0] as u32) << 24)
            | ((data[1] as u32) << 16)
            | ((data[2] as u32) << 8)
            | (data[3] as u32);
        let splice_event_cancel_indicator = (data[4] & 0x80) != 0;

        if splice_event_cancel_indicator {
            return Ok(SpliceInsert {
                splice_event_id,
                splice_event_cancel_indicator: true,
                out_of_network_indicator: false,
                program_splice_flag: false,
                splice_immediate_flag: false,
                splice_time: None,
                duration: None,
                unique_program_id: 0,
                avail_num: 0,
                avails_expected: 0,
            });
        }

        if data.len() < 6 {
            return Err(TsError::InvalidScte35(
                "splice_insert missing flags".to_string(),
            ));
        }

        let flags = data[5];
        let out_of_network_indicator = (flags & 0x80) != 0;
        let program_splice_flag = (flags & 0x40) != 0;
        let duration_flag = (flags & 0x20) != 0;
        let splice_immediate_flag = (flags & 0x10) != 0;

        let mut offset = 6;
        let mut splice_time = None;

        if program_splice_flag && !splice_immediate_flag {
            let (time, consumed) = parse_splice_time(&data[offset..]);
            splice_time = time;
            offset += consumed;
        }

        let duration = if duration_flag && offset + 5 <= data.len() {
            let bd = parse_break_duration(&data[offset..]);
            offset += 5;
            bd
        } else {
            None
        };

        let (unique_program_id, avail_num, avails_expected) = if offset + 4 <= data.len() {
            let upid = ((data[offset] as u16) << 8) | data[offset + 1] as u16;
            let an = data[offset + 2];
            let ae = data[offset + 3];
            (upid, an, ae)
        } else {
            (0, 0, 0)
        };

        Ok(SpliceInsert {
            splice_event_id,
            splice_event_cancel_indicator: false,
            out_of_network_indicator,
            program_splice_flag,
            splice_immediate_flag,
            splice_time,
            duration,
            unique_program_id,
            avail_num,
            avails_expected,
        })
    }
}

/// Zero-copy SCTE-35 splice info section reference.
#[derive(Debug, Clone)]
pub struct SpliceInfoSectionRef {
    #[allow(dead_code)]
    data: Bytes,
    pub inner: SpliceInfoSection,
}

impl SpliceInfoSectionRef {
    /// Parse from Bytes.
    pub fn parse(data: Bytes) -> Result<Self> {
        let inner = SpliceInfoSection::parse(&data)?;
        Ok(SpliceInfoSectionRef { data, inner })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_scte35_time_signal(pts: u64) -> Vec<u8> {
        let mut data = vec![
            0xFC, // table_id
            0x30, // section_syntax_indicator=0, private=0, reserved=11, section_length high
            0x11, // section_length low (17 bytes)
            0x00, // protocol_version=0
            0x00, // encrypted=0, encryption_algorithm=0, pts_adjustment high
        ];
        // pts_adjustment = 0 (remaining 4 bytes)
        data.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
        data.push(0x00); // cw_index
                         // tier (12 bits) + splice_command_length (12 bits)
                         // tier=0xFFF (all 1s), splice_command_length=5
        data.push(0xFF); // tier high 8
        data.push(0xF0); // tier low 4 | cmd_length high 4
        data.push(0x05); // cmd_length low 8
        data.push(0x06); // splice_command_type = time_signal

        // splice_time: time_specified=1, pts value
        data.push(0x80 | ((pts >> 32) as u8 & 0x01)); // time_specified + pts bit 32
        data.push((pts >> 24) as u8);
        data.push((pts >> 16) as u8);
        data.push((pts >> 8) as u8);
        data.push(pts as u8);

        // CRC placeholder
        data.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
        data
    }

    #[test]
    fn test_scte35_time_signal() {
        let data = make_scte35_time_signal(90000);
        let section = SpliceInfoSection::parse(&data).unwrap();
        assert_eq!(section.table_id, SCTE35_TABLE_ID);
        assert_eq!(section.protocol_version, 0);
        assert!(!section.encrypted_packet);
        assert_eq!(section.splice_command_type, SpliceCommandType::TimeSignal);
        match &section.splice_command {
            SpliceCommand::TimeSignal(ts) => {
                assert_eq!(ts.splice_time, Some(90000));
            }
            other => panic!("Expected TimeSignal, got {other:?}"),
        }
    }

    #[test]
    fn test_scte35_splice_null() {
        let mut data = vec![
            0xFC, // table_id
            0x30, 0x0E, // section_length=14
            0x00, // protocol_version
            0x00, 0x00, 0x00, 0x00, 0x00, // pts_adjustment=0
            0x00, // cw_index
            0xFF, 0xF0, 0x00, // tier=0xFFF, cmd_length=0
            0x00, // splice_command_type = splice_null
        ];
        // CRC placeholder
        data.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
        let section = SpliceInfoSection::parse(&data).unwrap();
        assert_eq!(section.splice_command_type, SpliceCommandType::SpliceNull);
        assert!(matches!(section.splice_command, SpliceCommand::SpliceNull));
    }

    #[test]
    fn test_scte35_splice_insert() {
        let mut data = vec![
            0xFC, // table_id
            0x30, 0x1A, // section_length
            0x00, // protocol_version
            0x00, 0x00, 0x00, 0x00, 0x00, // pts_adjustment=0
            0x00, // cw_index
            0xFF, 0xF0, 0x10, // tier=0xFFF, cmd_length=16
            0x05, // splice_command_type = splice_insert
        ];
        // splice_insert command data
        data.extend_from_slice(&[
            0x00, 0x00, 0x00, 0x01, // splice_event_id=1
            0x00, // splice_event_cancel_indicator=0, reserved
            0xE0, // out_of_network=1, program_splice=1, duration_flag=1, splice_immediate=0
        ]);
        // splice_time: time_specified=1, pts=0
        data.extend_from_slice(&[0x80, 0x00, 0x00, 0x00, 0x00]);
        // break_duration: auto_return=1, duration=2700000 (30 sec at 90kHz)
        let dur: u64 = 2_700_000;
        data.push(0x80 | ((dur >> 32) as u8 & 0x01));
        data.push((dur >> 24) as u8);
        data.push((dur >> 16) as u8);
        data.push((dur >> 8) as u8);
        data.push(dur as u8);

        // CRC placeholder
        data.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);

        let section = SpliceInfoSection::parse(&data).unwrap();
        assert_eq!(section.splice_command_type, SpliceCommandType::SpliceInsert);
        match &section.splice_command {
            SpliceCommand::SpliceInsert(insert) => {
                assert_eq!(insert.splice_event_id, 1);
                assert!(!insert.splice_event_cancel_indicator);
                assert!(insert.out_of_network_indicator);
                assert!(insert.program_splice_flag);
                assert!(!insert.splice_immediate_flag);
                assert_eq!(insert.splice_time, Some(0));
                let bd = insert.duration.unwrap();
                assert!(bd.auto_return);
                assert_eq!(bd.duration, 2_700_000);
            }
            other => panic!("Expected SpliceInsert, got {other:?}"),
        }
    }

    #[test]
    fn test_scte35_invalid_table_id() {
        let data = vec![0x00; 20];
        assert!(SpliceInfoSection::parse(&data).is_err());
    }

    #[test]
    fn test_scte35_ref() {
        let data = make_scte35_time_signal(45000);
        let section_ref = SpliceInfoSectionRef::parse(Bytes::from(data)).unwrap();
        assert_eq!(
            section_ref.inner.splice_command_type,
            SpliceCommandType::TimeSignal
        );
    }
}
