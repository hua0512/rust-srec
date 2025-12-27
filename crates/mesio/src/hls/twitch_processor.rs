use m3u8_rs::{MediaPlaylist, MediaSegment};
use std::collections::HashMap;
use tracing::debug;

pub(super) struct ProcessedSegment<'a> {
    pub segment: &'a MediaSegment,
    pub is_ad: bool,
}

#[derive(Debug, Clone)]
struct AdDateRange {
    start_ms: i64,
    end_ms: i64,
}

pub(super) struct TwitchPlaylistProcessor {
    ad_dateranges: HashMap<String, AdDateRange>,
    pub discontinuity: bool,
}

impl TwitchPlaylistProcessor {
    pub(super) fn new() -> Self {
        Self {
            ad_dateranges: HashMap::new(),
            discontinuity: false,
        }
    }

    #[inline]
    pub(super) fn is_twitch_playlist(base_url: &str) -> bool {
        base_url.contains("ttvnw.net")
    }

    pub(super) fn process_playlist<'a>(
        &mut self,
        playlist: &'a MediaPlaylist,
    ) -> Vec<ProcessedSegment<'a>> {
        let mut processed_segments = Vec::with_capacity(playlist.segments.len());

        for segment in &playlist.segments {
            if let Some(daterange) = &segment.daterange
                && (daterange.class.as_deref() == Some("twitch-stitched-ad")
                    || daterange.id.starts_with("stitched-ad-"))
                && let Some(end_date) = daterange.end_date
            {
                let is_new = self
                    .ad_dateranges
                    .insert(
                        daterange.id.clone(),
                        AdDateRange {
                            start_ms: daterange.start_date.timestamp_millis(),
                            end_ms: end_date.timestamp_millis(),
                        },
                    )
                    .is_none();

                if is_new {
                    debug!(
                        "New ad DATERANGE detected: id={}, class={:?}",
                        daterange.id, daterange.class
                    );
                }
            }
        }

        // Prune expired ad ranges to avoid unbounded growth on long streams.
        // Safe heuristic: if an ad ended before the earliest PDT in the current playlist window,
        // it cannot match any segment we'll consider again.
        let min_pdt_ms = playlist
            .segments
            .iter()
            .filter_map(|s| s.program_date_time.map(|pdt| pdt.timestamp_millis()))
            .min();
        if let Some(min_pdt_ms) = min_pdt_ms {
            self.ad_dateranges.retain(|_id, dr| dr.end_ms >= min_pdt_ms);
        }

        for segment in &playlist.segments {
            let mut is_ad = false;

            if let Some(pdt) = segment.program_date_time
                && self.ad_dateranges.values().any(|dr| {
                    let pdt_ms = pdt.timestamp_millis();
                    pdt_ms >= dr.start_ms && pdt_ms < dr.end_ms
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

            processed_segments.push(ProcessedSegment { segment, is_ad });
        }

        processed_segments
    }
}
