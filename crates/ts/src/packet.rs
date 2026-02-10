use crate::{Result, TsError};
use bytes::Bytes;

/// PAT PID (always 0x0000)
pub const PID_PAT: u16 = 0x0000;

/// NULL PID (always 0x1FFF)
pub const PID_NULL: u16 = 0x1FFF;

/// CAT PID (always 0x0001)
pub const PID_CAT: u16 = 0x0001;

/// Continuity counter status for a packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContinuityStatus {
    /// First packet seen for this PID
    Initial,
    /// Continuity is correct
    Ok,
    /// Discontinuity detected
    Discontinuity { expected: u8, actual: u8 },
    /// Duplicate packet (same CC as previous)
    Duplicate,
}

/// Continuity counter handling mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ContinuityMode {
    /// Do not evaluate continuity counters.
    #[default]
    Disabled,
    /// Validate continuity counters and continue parsing while reporting issues.
    Warn,
    /// Validate continuity counters and fail parsing on the first issue.
    Strict,
}

/// Transport Stream packet structure
#[derive(Debug, Clone)]
pub struct TsPacket {
    /// Sync byte (should always be 0x47)
    pub sync_byte: u8,
    /// Transport Error Indicator
    pub transport_error_indicator: bool,
    /// Payload Unit Start Indicator
    pub payload_unit_start_indicator: bool,
    /// Transport Priority
    pub transport_priority: bool,
    /// Packet Identifier
    pub pid: u16,
    /// Transport Scrambling Control
    pub transport_scrambling_control: u8,
    /// Adaptation Field Control
    pub adaptation_field_control: u8,
    /// Continuity Counter
    pub continuity_counter: u8,
    /// Adaptation field data (if present)
    pub adaptation_field: Option<Bytes>,
    /// Payload data (if present)
    pub payload: Option<Bytes>,
}

impl TsPacket {
    /// Parse a TS packet from 188 bytes
    pub fn parse(data: Bytes) -> Result<Self> {
        if data.len() != 188 {
            return Err(TsError::InvalidPacketSize(data.len()));
        }

        let sync_byte = data[0];
        if sync_byte != 0x47 {
            return Err(TsError::InvalidSyncByte(sync_byte));
        }

        let byte1 = data[1];
        let byte2 = data[2];
        let byte3 = data[3];

        let transport_error_indicator = (byte1 & 0x80) != 0;
        let payload_unit_start_indicator = (byte1 & 0x40) != 0;
        let transport_priority = (byte1 & 0x20) != 0;
        let pid = ((byte1 as u16 & 0x1F) << 8) | byte2 as u16;

        let transport_scrambling_control = (byte3 >> 6) & 0x03;
        let adaptation_field_control = (byte3 >> 4) & 0x03;
        let continuity_counter = byte3 & 0x0F;

        let mut offset = 4;
        let mut adaptation_field = None;
        let mut payload = None;

        // Parse adaptation field if present
        if adaptation_field_control == 0x02 || adaptation_field_control == 0x03 {
            if offset >= data.len() {
                return Err(TsError::InsufficientData {
                    expected: offset + 1,
                    actual: data.len(),
                });
            }

            let adaptation_field_length = data[offset] as usize;
            offset += 1;

            if adaptation_field_length > 0 {
                if offset + adaptation_field_length > data.len() {
                    return Err(TsError::InsufficientData {
                        expected: offset + adaptation_field_length,
                        actual: data.len(),
                    });
                }

                adaptation_field = Some(data.slice(offset..offset + adaptation_field_length));
                offset += adaptation_field_length;
            }
        }

        // Parse payload if present
        if (adaptation_field_control == 0x01 || adaptation_field_control == 0x03)
            && offset < data.len()
        {
            payload = Some(data.slice(offset..));
        }

        Ok(TsPacket {
            sync_byte,
            transport_error_indicator,
            payload_unit_start_indicator,
            transport_priority,
            pid,
            transport_scrambling_control,
            adaptation_field_control,
            continuity_counter,
            adaptation_field,
            payload,
        })
    }

    /// Check if this packet has a payload
    pub fn has_payload(&self) -> bool {
        self.adaptation_field_control == 0x01 || self.adaptation_field_control == 0x03
    }

    /// Check if this packet has an adaptation field
    pub fn has_adaptation_field(&self) -> bool {
        self.adaptation_field_control == 0x02 || self.adaptation_field_control == 0x03
    }

    /// Check if this packet contains a random access indicator
    pub fn has_random_access_indicator(&self) -> bool {
        if let Some(adaptation_field) = &self.adaptation_field
            && !adaptation_field.is_empty()
        {
            // Random access indicator is bit 6 (0x40) of the first byte
            return (adaptation_field[0] & 0x40) != 0;
        }
        false
    }

    /// Get the PSI payload (removes pointer field if PUSI is set)
    pub fn get_psi_payload(&self) -> Option<Bytes> {
        if let Some(payload) = &self.payload {
            if self.payload_unit_start_indicator && !payload.is_empty() {
                // Skip pointer field
                let pointer_field = payload[0] as usize;
                if 1 + pointer_field < payload.len() {
                    return Some(payload.slice(1 + pointer_field..));
                }
            } else if !self.payload_unit_start_indicator {
                // Continuation packet, return payload as-is
                return Some(payload.clone());
            }
        }
        None
    }

    /// Parse the adaptation field into a structured type.
    pub fn parse_adaptation_field(&self) -> Option<crate::adaptation_field::AdaptationField> {
        self.adaptation_field
            .as_ref()
            .and_then(|af| crate::adaptation_field::AdaptationField::parse(af))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_invalid_sync_byte() {
        let mut data = vec![0u8; 188];
        data[0] = 0x46; // Wrong sync byte
        assert!(TsPacket::parse(data.into()).is_err());
    }

    #[test]
    fn test_valid_packet_parsing() {
        let mut data = vec![0u8; 188];
        data[0] = 0x47; // Sync byte
        data[1] = 0x00; // No error, no PUSI, no priority, PID high = 0
        data[2] = 0x00; // PID low = 0 (PAT)
        data[3] = 0x10; // No scrambling, payload only, continuity = 0

        let packet = TsPacket::parse(data.into()).unwrap();
        assert_eq!(packet.sync_byte, 0x47);
        assert_eq!(packet.pid, 0);
        assert!(!packet.transport_error_indicator);
        assert!(!packet.payload_unit_start_indicator);
        assert!(!packet.transport_priority);
        assert_eq!(packet.transport_scrambling_control, 0);
        assert_eq!(packet.adaptation_field_control, 1);
        assert_eq!(packet.continuity_counter, 0);
        assert!(packet.has_payload());
        assert!(!packet.has_adaptation_field());
    }
}
