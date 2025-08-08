use m3u8_rs::{MediaPlaylist, MediaSegment};
use std::collections::HashSet;
use tracing::debug;

pub(super) struct ProcessedSegment {
    pub segment: MediaSegment,
    pub is_ad: bool,
}

pub(super) struct TwitchPlaylistProcessor {
    pub ad_dateranges: HashSet<String>,
    pub discontinuity: bool,
}

impl TwitchPlaylistProcessor {
    pub(super) fn new() -> Self {
        Self {
            ad_dateranges: HashSet::new(),
            discontinuity: false,
        }
    }

    #[inline]
    pub(super) fn is_twitch_playlist(base_url: &str) -> bool {
        base_url.contains("ttvnw.net")
    }

    pub(super) fn process_playlist(&mut self, playlist: &MediaPlaylist) -> Vec<ProcessedSegment> {
        let mut processed_segments = Vec::new();

        for segment in &playlist.segments {
            if let Some(daterange) = &segment.daterange
                && (daterange.class.as_deref() == Some("twitch-stitched-ad")
                    || daterange.id.starts_with("stitched-ad-"))
                && self.ad_dateranges.insert(daterange.id.clone())
            {
                debug!(
                    "New ad DATERANGE detected: id={}, class={:?}",
                    daterange.id, daterange.class
                );
            }
        }

        for segment in &playlist.segments {
            let mut is_ad = false;

            if let Some(pdt) = segment.program_date_time
                && self.ad_dateranges.iter().any(|ad_id| {
                    playlist.segments.iter().any(|s| {
                        s.daterange.as_ref().is_some_and(|dr| {
                            &dr.id == ad_id && pdt >= dr.start_date && pdt < dr.end_date.unwrap()
                        })
                    })
                })
            {
                is_ad = true;
            }

            if segment.discontinuity {
                self.discontinuity = true;
            } else if self.discontinuity {
                // Heuristic: the first segment after a discontinuity is a prefetch ad
                if segment.title.as_deref() == Some("PREFETCH_SEGMENT") {
                    is_ad = true;
                }
                self.discontinuity = false;
            }

            processed_segments.push(ProcessedSegment {
                segment: segment.clone(),
                is_ad,
            });
        }

        processed_segments
    }
}
