use std::io;

use bytes_util::{BitReader, range_check};
use expgolomb::BitReaderExpGolombExt;

/// `ScalingList[0][0..5][i]`
///
/// ISO/IEC 23008-2 - Table 7-5
const TABLE_7_5: [i64; 16] = [16; 16];

/// `ScalingList[1..3][0..2][i]`
///
/// /// ISO/IEC 23008-2 - Table 7-6
#[rustfmt::skip]
const TABLE_7_6_02: [i64; 64] = [
    //0  1   2   3   4   5   6   7   8   9  10  11  12  13  14  15
    16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 17, 16, 17, 16, 17, 18,
    17, 18, 18, 17, 18, 21, 19, 20, 21, 20, 19, 21, 24, 22, 22, 24,
    24, 22, 22, 24, 25, 25, 27, 30, 27, 25, 25, 29, 31, 35, 35, 31,
    29, 36, 41, 44, 41, 36, 47, 54, 54, 47, 65, 70, 65, 88, 88, 115,
];

/// `ScalingList[1..3][3..5][i]`
///
/// ISO/IEC 23008-2 - Table 7-6
#[rustfmt::skip]
const TABLE_7_6_35: [i64; 64] = [
    //0  1   2   3   4   5   6   7   8   9  10  11  12  13  14  15
    16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 17, 17, 17, 17, 17, 18,
    18, 18, 18, 18, 18, 20, 20, 20, 20, 20, 20, 20, 24, 24, 24, 24,
    24, 24, 24, 24, 25, 25, 25, 25, 25, 25, 25, 28, 28, 28, 28, 28,
    28, 33, 33, 33, 33, 33, 41, 41, 41, 41, 54, 54, 54, 71, 71, 91,
];

/// `ScalingList[1..3][i][j]`
///
/// ISO/IEC 23008-2 - 7.4.5
const TABLE_7_6: [[i64; 64]; 6] = [
    TABLE_7_6_02,
    TABLE_7_6_02,
    TABLE_7_6_02,
    TABLE_7_6_35,
    TABLE_7_6_35,
    TABLE_7_6_35,
];

/// Scaling list data.
///
/// `scaling_list_data()`
///
/// - ISO/IEC 23008-2 - 7.3.4
/// - ISO/IEC 23008-2 - 7.4.5
#[derive(Debug, Clone, PartialEq)]
pub struct ScalingListData {
    /// The resulting scaling list.
    ///
    /// `ScalingList[0..3][0..5][0..63]`
    pub scaling_list: [[[i64; 64]; 6]; 4],
}

impl ScalingListData {
    pub(crate) fn parse<R: io::Read>(bit_reader: &mut BitReader<R>) -> io::Result<Self> {
        let mut scaling_list = [[[0; 64]; 6]; 4];

        for (size_id, scaling_column) in scaling_list.iter_mut().enumerate() {
            let coef_num = usize::min(64, 1 << (4 + (size_id << 1)));
            let step = if size_id == 3 { 3 } else { 1 };
            let mut matrix_id = 0;

            while matrix_id < 6 {
                let scaling_list_pred_mode_flag = bit_reader.read_bit()?;

                if !scaling_list_pred_mode_flag {
                    // The values of the scaling list are inferred from a default or reference scaling list.
                    let scaling_list_pred_matrix_id_delta = bit_reader.read_exp_golomb()?;

                    if scaling_list_pred_matrix_id_delta == 0 {
                        if size_id == 0 {
                            scaling_column[matrix_id][0..16].copy_from_slice(&TABLE_7_5);
                        } else {
                            scaling_column[matrix_id][0..coef_num]
                                .copy_from_slice(&TABLE_7_6[matrix_id][0..coef_num]);
                        }
                    } else {
                        let ref_matrix_id = usize::try_from(scaling_list_pred_matrix_id_delta)
                            .ok()
                            .and_then(|delta| delta.checked_mul(step))
                            .and_then(|offset| matrix_id.checked_sub(offset))
                            .ok_or_else(|| {
                                io::Error::new(
                                    io::ErrorKind::InvalidData,
                                    "scaling_list_pred_matrix_id_delta references out-of-range matrix_id",
                                )
                            })?;

                        let (prev_matrices, current_and_rest) =
                            scaling_column.split_at_mut(matrix_id);
                        current_and_rest[0][0..coef_num]
                            .copy_from_slice(&prev_matrices[ref_matrix_id][0..coef_num]);
                    }
                } else {
                    // The values of the scaling list are explicitly signalled.
                    let mut next_coef = 8;

                    if size_id > 1 {
                        let scaling_list_dc_coef_minus8 = bit_reader.read_signed_exp_golomb()?;
                        range_check!(scaling_list_dc_coef_minus8, -7, 247)?;
                        next_coef = scaling_list_dc_coef_minus8 + 8;
                    }

                    for coef in scaling_column[matrix_id].iter_mut().take(coef_num) {
                        let scaling_list_delta_coef = bit_reader.read_signed_exp_golomb()?;
                        range_check!(scaling_list_delta_coef, -128, 127)?;
                        next_coef = (next_coef + scaling_list_delta_coef + 256) % 256;
                        *coef = next_coef;
                    }
                }

                matrix_id += step;
            }
        }

        Ok(Self { scaling_list })
    }
}

#[cfg(test)]
#[cfg_attr(all(test, coverage_nightly), coverage(off))]
mod tests {
    use std::io;

    use bytes_util::{BitReader, BitWriter};
    use expgolomb::BitWriterExpGolombExt;

    use super::ScalingListData;

    #[test]
    fn rejects_out_of_bounds_pred_matrix_id_delta() {
        let mut buf = Vec::new();
        let mut writer = BitWriter::new(&mut buf);
        // size_id = 0, matrix_id = 0: pred_mode_flag = false, delta = 1 (references matrix_id -1)
        writer.write_bit(false).unwrap();
        writer.write_exp_golomb(1).unwrap();
        writer.finish().unwrap();

        let err = ScalingListData::parse(&mut BitReader::new(buf.as_slice())).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn prediction_copies_previous_decoded_matrix_and_64th_coeff() {
        let mut buf = Vec::new();
        let mut writer = BitWriter::new(&mut buf);

        // size_id = 0 (16 coeffs): matrix 0 explicit (all 9s), matrix 1 predicted from matrix 0 (delta = 1),
        // matrices 2..6 default (delta = 0).
        writer.write_bit(true).unwrap();
        writer.write_signed_exp_golomb(1).unwrap(); // 8 + 1 = 9
        for _ in 1..16 {
            writer.write_signed_exp_golomb(0).unwrap();
        }
        writer.write_bit(false).unwrap();
        writer.write_exp_golomb(1).unwrap(); // copy matrix 0
        for _ in 2..6 {
            writer.write_bit(false).unwrap();
            writer.write_exp_golomb(0).unwrap();
        }

        // size_id = 1 (64 coeffs): matrix 0 explicit (all 23s), matrix 1 predicted from matrix 0 (delta = 1),
        // matrices 2..6 default (delta = 0).
        writer.write_bit(true).unwrap();
        writer.write_signed_exp_golomb(15).unwrap(); // 8 + 15 = 23
        for _ in 1..64 {
            writer.write_signed_exp_golomb(0).unwrap();
        }
        writer.write_bit(false).unwrap();
        writer.write_exp_golomb(1).unwrap(); // copy matrix 0
        for _ in 2..6 {
            writer.write_bit(false).unwrap();
            writer.write_exp_golomb(0).unwrap();
        }

        // size_id = 2 (6 matrices) and size_id = 3 (2 matrices): all default (delta = 0).
        for _ in 0..(6 + 2) {
            writer.write_bit(false).unwrap();
            writer.write_exp_golomb(0).unwrap();
        }
        writer.finish().unwrap();

        let parsed = ScalingListData::parse(&mut BitReader::new(buf.as_slice())).unwrap();
        assert_eq!(&parsed.scaling_list[0][0][..16], &[9; 16]);
        assert_eq!(&parsed.scaling_list[0][1][..16], &[9; 16]);
        assert_eq!(parsed.scaling_list[1][0], [23; 64]);
        assert_eq!(parsed.scaling_list[1][1], [23; 64]);
        // Default 8x8 matrix 2 must include its 64th element (115 in Table 7-6).
        assert_eq!(parsed.scaling_list[1][2][63], 115);
    }

    #[test]
    fn rejects_out_of_range_dc_and_delta_coefficients() {
        // Out-of-range delta_coef (128 > 127) in size_id = 0, matrix_id = 0.
        let mut buf = Vec::new();
        let mut writer = BitWriter::new(&mut buf);
        writer.write_bit(true).unwrap();
        writer.write_signed_exp_golomb(128).unwrap();
        writer.finish().unwrap();
        let err = ScalingListData::parse(&mut BitReader::new(buf.as_slice())).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);

        // Out-of-range dc_coef_minus8 (-8 < -7) in size_id = 2, matrix_id = 0.
        let mut buf = Vec::new();
        let mut writer = BitWriter::new(&mut buf);
        for _ in 0..12 {
            writer.write_bit(false).unwrap();
            writer.write_exp_golomb(0).unwrap();
        }
        writer.write_bit(true).unwrap();
        writer.write_signed_exp_golomb(-8).unwrap();
        writer.finish().unwrap();
        let err = ScalingListData::parse(&mut BitReader::new(buf.as_slice())).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }
}
