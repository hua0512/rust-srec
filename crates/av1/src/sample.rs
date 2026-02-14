//! AV1 sample payload parsing helpers for container formats.
//!
//! This module focuses on ISOBMFF/MP4 sample payload semantics.
//! An AV1 sample payload is a sequence of OBUs forming one temporal unit,
//! where the last OBU may omit `obu_has_size_field`.
//!
//! The parser uses container-tolerant OBU iteration and can enforce AV1
//! ISOBMFF conformance checks for OBU types that are disallowed or discouraged
//! by the AV1 ISOBMFF binding.

use std::io;

use bytes::Bytes;

use crate::error::{Av1Error, Result};
use crate::obu::ObuType;
use crate::obu_stream::{ContainerObuIterator, Obu};

/// Parsing options for AV1 ISOBMFF sample payloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IsobmffSampleParseOptions {
    /// If `true`, reject OBU types marked as "SHOULD NOT" in AV1 ISOBMFF.
    ///
    /// Specifically rejects:
    /// - `OBU_TEMPORAL_DELIMITER`
    /// - `OBU_PADDING`
    /// - `OBU_REDUNDANT_FRAME_HEADER`
    pub enforce_should_not_obus: bool,
    /// If `true`, reject reserved OBU types.
    pub enforce_reserved_obus: bool,
}

impl Default for IsobmffSampleParseOptions {
    fn default() -> Self {
        Self {
            enforce_should_not_obus: true,
            enforce_reserved_obus: false,
        }
    }
}

/// Parses an AV1 ISOBMFF sample payload into OBUs with conformance checks enabled.
///
/// This parser allows the last OBU to omit `obu_has_size_field` and enforces
/// OBU-type conformance according to [`IsobmffSampleParseOptions::default`].
pub fn parse_isobmff_sample(data: &Bytes) -> Result<Vec<Obu>> {
    parse_isobmff_sample_with_options(data, IsobmffSampleParseOptions::default())
}

/// Parses an AV1 ISOBMFF sample payload into OBUs with configurable conformance checks.
pub fn parse_isobmff_sample_with_options(
    data: &Bytes,
    options: IsobmffSampleParseOptions,
) -> Result<Vec<Obu>> {
    let mut cursor = io::Cursor::new(data.clone());
    let mut obus = Vec::new();

    for obu in ContainerObuIterator::new(&mut cursor) {
        let obu = obu?;
        validate_isobmff_obu_type(obu.header.obu_type, options)?;
        obus.push(obu);
    }

    Ok(obus)
}

/// Validates an AV1 ISOBMFF sample payload without allocating parsed OBU storage.
///
/// This parser allows the last OBU to omit `obu_has_size_field` and enforces
/// OBU-type conformance according to [`IsobmffSampleParseOptions::default`].
pub fn validate_isobmff_sample(data: &Bytes) -> Result<()> {
    validate_isobmff_sample_with_options(data, IsobmffSampleParseOptions::default())
}

/// Validates an AV1 ISOBMFF sample payload without allocating parsed OBU storage,
/// with configurable conformance checks.
pub fn validate_isobmff_sample_with_options(
    data: &Bytes,
    options: IsobmffSampleParseOptions,
) -> Result<()> {
    validate_isobmff_sample_bytes_with_options(data.as_ref(), options)
}

/// Validates an AV1 ISOBMFF sample payload from a byte slice without allocations.
pub fn validate_isobmff_sample_bytes(data: &[u8]) -> Result<()> {
    validate_isobmff_sample_bytes_with_options(data, IsobmffSampleParseOptions::default())
}

/// Validates an AV1 ISOBMFF sample payload from a byte slice without allocations,
/// with configurable conformance checks.
pub fn validate_isobmff_sample_bytes_with_options(
    data: &[u8],
    options: IsobmffSampleParseOptions,
) -> Result<()> {
    use crate::ObuHeader;

    let mut cursor = io::Cursor::new(data);
    while (cursor.position() as usize) < data.len() {
        let header = ObuHeader::parse(&mut cursor)?;
        validate_isobmff_obu_type(header.obu_type, options)?;

        let payload_size = header
            .size
            .unwrap_or_else(|| data.len() as u64 - cursor.position())
            as usize;
        let next = cursor.position() as usize + payload_size;
        if next > data.len() {
            return Err(Av1Error::UnexpectedEof {
                expected: payload_size,
                actual: data.len().saturating_sub(cursor.position() as usize),
            });
        }

        cursor.set_position(next as u64);
    }

    Ok(())
}

fn validate_isobmff_obu_type(obu_type: ObuType, options: IsobmffSampleParseOptions) -> Result<()> {
    match obu_type {
        ObuType::TileList => Err(Av1Error::InvalidObu(
            "OBU_TILE_LIST is not allowed in ISOBMFF samples".to_string(),
        )),
        ObuType::TemporalDelimiter if options.enforce_should_not_obus => Err(Av1Error::InvalidObu(
            "OBU_TEMPORAL_DELIMITER should not appear in ISOBMFF samples".to_string(),
        )),
        ObuType::Padding if options.enforce_should_not_obus => Err(Av1Error::InvalidObu(
            "OBU_PADDING should not appear in ISOBMFF samples".to_string(),
        )),
        ObuType::RedundantFrameHeader if options.enforce_should_not_obus => {
            Err(Av1Error::InvalidObu(
                "OBU_REDUNDANT_FRAME_HEADER should not appear in ISOBMFF samples".to_string(),
            ))
        }
        ObuType::Reserved(value) if options.enforce_reserved_obus => Err(Av1Error::InvalidObu(
            format!("Reserved OBU type {value} is not allowed in strict ISOBMFF samples"),
        )),
        _ => Ok(()),
    }
}

#[cfg(test)]
#[cfg_attr(all(coverage_nightly, test), coverage(off))]
mod tests {
    use super::*;
    use crate::ObuHeader;
    use crate::obu_stream::write_obu;

    #[test]
    fn test_parse_isobmff_sample_allows_unsized_last_obu() {
        let mut sample = Vec::new();
        write_obu(&mut sample, ObuType::Metadata, None, &[0x11]).unwrap();

        let unsized_last = ObuHeader {
            obu_type: ObuType::Frame,
            size: None,
            extension_header: None,
        };
        unsized_last.mux(&mut sample).unwrap();
        sample.extend_from_slice(&[0xAA, 0xBB, 0xCC]);

        let parsed = parse_isobmff_sample(&Bytes::from(sample)).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].header.obu_type, ObuType::Metadata);
        assert_eq!(parsed[0].data.as_ref(), &[0x11]);
        assert_eq!(parsed[1].header.obu_type, ObuType::Frame);
        assert_eq!(parsed[1].header.size, None);
        assert_eq!(parsed[1].data.as_ref(), &[0xAA, 0xBB, 0xCC]);
    }

    #[test]
    fn test_parse_isobmff_sample_rejects_tile_list() {
        let mut sample = Vec::new();
        write_obu(&mut sample, ObuType::TileList, None, &[0x00]).unwrap();

        let err = parse_isobmff_sample(&Bytes::from(sample)).unwrap_err();
        assert!(matches!(err, Av1Error::InvalidObu(msg) if msg.contains("OBU_TILE_LIST")));
    }

    #[test]
    fn test_parse_isobmff_sample_rejects_should_not_obu_by_default() {
        let mut sample = Vec::new();
        write_obu(&mut sample, ObuType::TemporalDelimiter, None, &[]).unwrap();

        let err = parse_isobmff_sample(&Bytes::from(sample)).unwrap_err();
        assert!(matches!(err, Av1Error::InvalidObu(msg) if msg.contains("OBU_TEMPORAL_DELIMITER")));
    }

    #[test]
    fn test_parse_isobmff_sample_allows_should_not_obu_when_disabled() {
        let mut sample = Vec::new();
        write_obu(&mut sample, ObuType::TemporalDelimiter, None, &[]).unwrap();

        let parsed = parse_isobmff_sample_with_options(
            &Bytes::from(sample),
            IsobmffSampleParseOptions {
                enforce_should_not_obus: false,
                enforce_reserved_obus: false,
            },
        )
        .unwrap();

        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].header.obu_type, ObuType::TemporalDelimiter);
    }

    #[test]
    fn test_validate_isobmff_sample_allows_unsized_last_obu() {
        let mut sample = Vec::new();
        write_obu(&mut sample, ObuType::Metadata, None, &[0x11]).unwrap();

        let unsized_last = ObuHeader {
            obu_type: ObuType::Frame,
            size: None,
            extension_header: None,
        };
        unsized_last.mux(&mut sample).unwrap();
        sample.extend_from_slice(&[0xAA, 0xBB, 0xCC]);

        validate_isobmff_sample(&Bytes::from(sample)).unwrap();
    }

    #[test]
    fn test_validate_isobmff_sample_rejects_disallowed_obu() {
        let mut sample = Vec::new();
        write_obu(&mut sample, ObuType::TileList, None, &[0x00]).unwrap();

        let err = validate_isobmff_sample(&Bytes::from(sample)).unwrap_err();
        assert!(matches!(err, Av1Error::InvalidObu(msg) if msg.contains("OBU_TILE_LIST")));
    }

    #[test]
    fn test_validate_isobmff_sample_bytes_rejects_reserved_when_strict_all() {
        let mut sample = Vec::new();
        write_obu(&mut sample, ObuType::Reserved(9), None, &[0x00]).unwrap();

        let err = validate_isobmff_sample_bytes_with_options(
            &sample,
            IsobmffSampleParseOptions {
                enforce_should_not_obus: true,
                enforce_reserved_obus: true,
            },
        )
        .unwrap_err();

        assert!(matches!(err, Av1Error::InvalidObu(msg) if msg.contains("Reserved OBU type")));
    }
}
