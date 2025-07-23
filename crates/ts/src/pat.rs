use crate::{Result, TsError};

/// Program Association Table (PAT) - Table ID 0x00
#[derive(Debug, Clone)]
pub struct Pat {
    /// Table ID (should be 0x00 for PAT)
    pub table_id: u8,
    /// Transport Stream ID
    pub transport_stream_id: u16,
    /// Version number
    pub version_number: u8,
    /// Current/next indicator
    pub current_next_indicator: bool,
    /// Section number
    pub section_number: u8,
    /// Last section number
    pub last_section_number: u8,
    /// List of programs
    pub programs: Vec<PatProgram>,
}

/// Program entry in PAT
#[derive(Debug, Clone)]
pub struct PatProgram {
    /// Program number (0 = Network PID, others = Program numbers)
    pub program_number: u16,
    /// PID of PMT (if program_number > 0) or Network PID (if program_number = 0)
    pub pmt_pid: u16,
}

impl Pat {
    /// Parse PAT from PSI section data
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < 8 {
            return Err(TsError::InsufficientData {
                expected: 8,
                actual: data.len(),
            });
        }

        let table_id = data[0];
        if table_id != 0x00 {
            return Err(TsError::InvalidTableId {
                expected: 0x00,
                actual: table_id,
            });
        }

        // Parse section header
        let section_syntax_indicator = (data[1] & 0x80) != 0;
        if !section_syntax_indicator {
            return Err(TsError::ParseError(
                "PAT must have section syntax indicator set".to_string(),
            ));
        }

        let section_length = ((data[1] as u16 & 0x0F) << 8) | data[2] as u16;
        if section_length < 5 {
            return Err(TsError::InvalidSectionLength(section_length));
        }

        if data.len() < (3 + section_length as usize) {
            return Err(TsError::InsufficientData {
                expected: 3 + section_length as usize,
                actual: data.len(),
            });
        }

        let transport_stream_id = ((data[3] as u16) << 8) | data[4] as u16;
        let version_number = (data[5] >> 1) & 0x1F;
        let current_next_indicator = (data[5] & 0x01) != 0;
        let section_number = data[6];
        let last_section_number = data[7];

        // Parse programs (each program entry is 4 bytes)
        let mut programs = Vec::new();
        let mut offset = 8;
        let programs_end = 3 + section_length as usize - 4; // Exclude CRC32

        while offset + 4 <= programs_end {
            let program_number = ((data[offset] as u16) << 8) | data[offset + 1] as u16;
            let pid = ((data[offset + 2] as u16 & 0x1F) << 8) | data[offset + 3] as u16;

            programs.push(PatProgram {
                program_number,
                pmt_pid: pid,
            });

            offset += 4;
        }

        // TODO: Verify CRC32 if needed
        // let crc32 = u32::from_be_bytes([
        //     data[programs_end],
        //     data[programs_end + 1],
        //     data[programs_end + 2],
        //     data[programs_end + 3],
        // ]);

        Ok(Pat {
            table_id,
            transport_stream_id,
            version_number,
            current_next_indicator,
            section_number,
            last_section_number,
            programs,
        })
    }

    /// Get the Network PID (program number 0)
    pub fn network_pid(&self) -> Option<u16> {
        self.programs
            .iter()
            .find(|p| p.program_number == 0)
            .map(|p| p.pmt_pid)
    }

    /// Get all program numbers (excluding network program)
    pub fn program_numbers(&self) -> Vec<u16> {
        self.programs
            .iter()
            .filter(|p| p.program_number != 0)
            .map(|p| p.program_number)
            .collect()
    }

    /// Get PMT PID for a specific program number
    pub fn get_pmt_pid(&self, program_number: u16) -> Option<u16> {
        self.programs
            .iter()
            .find(|p| p.program_number == program_number)
            .map(|p| p.pmt_pid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pat_invalid_table_id() {
        let data = vec![0x01, 0x80, 0x0D, 0x00, 0x01, 0x00, 0x00, 0x00];
        assert!(Pat::parse(&data).is_err());
    }

    #[test]
    fn test_pat_insufficient_data() {
        let data = vec![0x00, 0x80];
        assert!(Pat::parse(&data).is_err());
    }

    #[test]
    fn test_pat_basic_parsing() {
        // Example PAT with one program
        let data = vec![
            0x00, // Table ID
            0x80, // Section syntax indicator + section length high
            0x0D, // Section length low (13 bytes total)
            0x00,
            0x01, // Transport stream ID
            0x01, // Version 0 + current/next = 1
            0x00, // Section number
            0x00, // Last section number
            // Program 1
            0x00,
            0x01,        // Program number 1
            0xE0 | 0x10, // PMT PID high (0x1000)
            0x00,        // PMT PID low
            // CRC32 placeholder
            0x00,
            0x00,
            0x00,
            0x00,
        ];

        let pat = Pat::parse(&data).unwrap();
        assert_eq!(pat.table_id, 0x00);
        assert_eq!(pat.transport_stream_id, 0x0001);
        assert_eq!(pat.version_number, 0);
        assert!(pat.current_next_indicator);
        assert_eq!(pat.programs.len(), 1);
        assert_eq!(pat.programs[0].program_number, 1);
        assert_eq!(pat.programs[0].pmt_pid, 0x1000);
    }
}
