#![no_main]

use libfuzzer_sys::fuzz_target;
use mp4::sidx::{SegmentIndex, locate};

fuzz_target!(|data: &[u8]| {
    let _ = SegmentIndex::parse(data);
    if let Ok(Some(extent)) = locate(data)
        && let (Ok(start), Ok(end)) = (usize::try_from(extent.start), usize::try_from(extent.end))
        && let Some(body) = data.get(start..end)
    {
        let _ = SegmentIndex::parse(body);
    }
});
