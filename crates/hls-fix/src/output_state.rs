use hls::{HlsData, M4sData, M4sInitSegmentData};

/// The bytes a standalone HLS recording contains. Both the pipeline limiter and
/// the writer use this accounting, including implicit init replay and duplicate
/// init suppression. Boundaries are checked before media, never before headers
/// or control markers; a first media item is kept intact even if oversized.
#[derive(Default)]
pub(crate) struct OutputState {
    bytes: u64,
    media_written: bool,
    init: Option<M4sInitSegmentData>,
    init_written: bool,
}

impl OutputState {
    pub(crate) fn reset_file(&mut self) {
        self.bytes = 0;
        self.media_written = false;
        self.init_written = false;
    }

    pub(crate) fn has_media(&self) -> bool {
        self.media_written
    }

    pub(crate) fn pending_init(&self) -> Option<&M4sInitSegmentData> {
        self.init.as_ref().filter(|_| !self.init_written)
    }

    pub(crate) fn is_repeated_init(&self, init: &M4sInitSegmentData) -> bool {
        self.init_written
            && self
                .init
                .as_ref()
                .is_some_and(|previous| previous.data == init.data)
    }

    fn additional_bytes(&self, item: &HlsData) -> u64 {
        match item {
            HlsData::M4sData(M4sData::InitSegment(init)) if self.is_repeated_init(init) => 0,
            HlsData::M4sData(M4sData::Segment(segment)) => (segment.data.len() as u64)
                .saturating_add(self.pending_init().map_or(0, |init| init.data.len() as u64)),
            _ => item.size() as u64,
        }
    }

    pub(crate) fn would_exceed(&self, item: &HlsData, limit: Option<u64>) -> bool {
        self.media_written
            && matches!(
                item,
                HlsData::TsData(_) | HlsData::M4sData(M4sData::Segment(_))
            )
            && limit.is_some_and(|limit| {
                limit > 0
                    && self
                        .bytes
                        .checked_add(self.additional_bytes(item))
                        .is_none_or(|total| total > limit)
            })
    }

    /// Account for a data item and any required init replay. Returns its actual
    /// output size; call after a successful write, or before forwarding an item
    /// through a pipeline that aborts on an output error.
    pub(crate) fn record(&mut self, item: &HlsData) -> u64 {
        let added = self.additional_bytes(item);
        match item {
            HlsData::TsData(_) => {
                self.init = None;
                self.init_written = false;
                self.media_written = true;
            }
            HlsData::M4sData(M4sData::InitSegment(init)) => {
                self.init = Some(init.clone());
                self.init_written = true;
            }
            HlsData::M4sData(M4sData::Segment(_)) => {
                self.init_written = self.init.is_some();
                self.media_written = true;
            }
            HlsData::EndMarker(_) => {}
        }
        self.bytes = self.bytes.saturating_add(added);
        added
    }
}
