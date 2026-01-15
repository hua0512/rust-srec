use m3u8_rs::DateRange;
use m3u8_rs::{MediaPlaylist, MediaSegment};
use moka::sync::Cache;
use tracing::debug;

pub(super) struct ProcessedSegment<'a> {
    pub segment: &'a MediaSegment,
    pub is_ad: bool,
    pub discontinuity: bool,
}

#[derive(Debug, Clone)]
struct AdDateRange {
    start_ms: i64,
    end_ms: i64,
}

pub(super) struct TwitchPlaylistProcessor {
    ad_dateranges: Cache<String, AdDateRange>,
    pub discontinuity: bool,
}

#[inline]
fn is_prefetch_segment(segment: &MediaSegment) -> bool {
    segment.title.as_deref() == Some("PREFETCH_SEGMENT")
}

#[inline]
fn is_ad_title(title: Option<&str>) -> bool {
    title.is_some_and(|t| t.contains("Amazon"))
}

#[inline]
fn is_ad_daterange(daterange: &DateRange) -> bool {
    daterange.class.as_deref() == Some("twitch-stitched-ad")
        || daterange.id.starts_with("stitched-ad-")
}

#[inline]
fn daterange_end_ms(daterange: &DateRange) -> Option<i64> {
    if let Some(end) = daterange.end_date {
        return Some(end.timestamp_millis());
    }

    let start_ms = daterange.start_date.timestamp_millis();
    if let Some(duration) = daterange.duration {
        return Some(start_ms.saturating_add((duration * 1000.0).ceil() as i64));
    }
    if let Some(planned) = daterange.planned_duration {
        return Some(start_ms.saturating_add((planned * 1000.0).ceil() as i64));
    }

    None
}

impl TwitchPlaylistProcessor {
    pub(super) fn new() -> Self {
        Self {
            ad_dateranges: Cache::builder()
                .max_capacity(256)
                // Twitch playlists normally have PDT and get pruned explicitly
                // below. This capacity is a backstop for unexpected playlists
                // where pruning can't run (eg. missing PDT).
                .build(),
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

        // Twitch-specific prefetch segments are transformed into regular HLS segments by
        // preprocessing. Exclude them here when calculating average segment duration.
        let (sum_regular, count_regular) = playlist
            .segments
            .iter()
            .filter(|s| !is_prefetch_segment(s))
            .fold((0.0_f32, 0_usize), |(sum, count), s| {
                (sum + s.duration, count + 1)
            });
        let avg_regular_duration = if count_regular > 0 {
            Some(sum_regular / count_regular as f32)
        } else {
            None
        };

        for segment in &playlist.segments {
            if let Some(daterange) = &segment.daterange
                && is_ad_daterange(daterange)
                && let Some(end_ms) = daterange_end_ms(daterange)
            {
                let start_ms = daterange.start_date.timestamp_millis();
                let ad_range = AdDateRange { start_ms, end_ms };
                let prev = self.ad_dateranges.get(&daterange.id);
                let is_new_or_changed =
                    prev.is_none_or(|prev| prev.start_ms != start_ms || prev.end_ms != end_ms);
                self.ad_dateranges.insert(daterange.id.clone(), ad_range);

                if is_new_or_changed {
                    debug!(
                        "Ad DATERANGE detected: id={}, class={:?}",
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
            // Safe heuristic: if an ad ended before the earliest PDT in the current
            // playlist window, it cannot match any segment we'll consider again.
            let _ = self
                .ad_dateranges
                .invalidate_entries_if(move |_id, dr| dr.end_ms < min_pdt_ms);
        }

        let mut last_date_ms: Option<i64> = None;
        let mut last_duration_s: f32 = 0.0;
        let mut last_was_prefetch = false;
        let mut last_was_ad = false;

        for segment in &playlist.segments {
            let is_prefetch = is_prefetch_segment(segment);

            let segment_duration_s = if is_prefetch {
                if last_was_prefetch {
                    last_duration_s
                } else {
                    avg_regular_duration.unwrap_or(segment.duration)
                }
            } else {
                segment.duration
            };

            let segment_date_ms = if let Some(pdt) = segment.program_date_time {
                Some(pdt.timestamp_millis())
            } else if is_prefetch {
                last_date_ms.map(|ms| ms.saturating_add((last_duration_s * 1000.0).round() as i64))
            } else {
                None
            };

            let mut is_ad = false;

            // Streamlink twitch.py plugin logic:
            // - ad segments have an EXTINF title containing "Amazon"
            // - and/or segments fall into stitched-ad dateranges
            if is_ad_title(segment.title.as_deref()) {
                is_ad = true;
            } else if segment.daterange.as_ref().is_some_and(is_ad_daterange) {
                // Mark the segment which carries the stitched-ad daterange tag as an ad too.
                is_ad = true;
            } else if let Some(ms) = segment_date_ms
                && self
                    .ad_dateranges
                    .iter()
                    .any(|(_id, dr)| ms >= dr.start_ms && ms < dr.end_ms)
            {
                is_ad = true;
            }

            // Special case where Twitch incorrectly inserts discontinuity tags between
            // segments of the live content (Streamlink logic).
            //
            // Apply this only to non-prefetch segments. For prefetch segments, a
            // discontinuity tag is an important signal for ad detection.
            let effective_discontinuity =
                if segment.discontinuity && !is_prefetch && !is_ad && !last_was_ad {
                    false
                } else {
                    segment.discontinuity
                };

            if effective_discontinuity {
                self.discontinuity = true;
            }

            // Prefetch segments after a discontinuity should always be treated as ads.
            // Don't reset the discontinuity state here: date extrapolation can be inaccurate
            // and we want to treat all subsequent prefetch segments as ads until real content
            // resumes (Streamlink logic).
            if is_prefetch && self.discontinuity {
                is_ad = true;
            } else if !is_prefetch && !is_ad {
                self.discontinuity = false;
            }

            let discontinuity = if is_prefetch {
                // Streamlink twitch.py: set prefetch discontinuity based on ad transitions.
                is_ad != last_was_ad
            } else {
                // Ensure a discontinuity is observable on the first non-ad segment after ads,
                // even if Twitch only marked the skipped ad segments with a discontinuity tag.
                effective_discontinuity || (!is_ad && last_was_ad)
            };

            processed_segments.push(ProcessedSegment {
                segment,
                is_ad,
                discontinuity,
            });

            if let Some(ms) = segment_date_ms {
                last_date_ms = Some(ms);
            }
            last_duration_s = segment_duration_s;
            last_was_prefetch = is_prefetch;
            last_was_ad = is_ad;
        }

        processed_segments
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use m3u8_rs::parse_playlist_res;

    fn parse_media_playlist(input: &str) -> MediaPlaylist {
        match parse_playlist_res(input.as_bytes()).expect("playlist should parse") {
            m3u8_rs::Playlist::MediaPlaylist(pl) => pl,
            m3u8_rs::Playlist::MasterPlaylist(_) => panic!("expected media playlist"),
        }
    }

    #[test]
    fn detects_stitched_ads_via_daterange_duration_and_title() {
        let playlist = parse_media_playlist(
            "#EXTM3U\n\
#EXT-X-VERSION:7\n\
#EXT-X-TARGETDURATION:2\n\
#EXT-X-MEDIA-SEQUENCE:1\n\
#EXT-X-PROGRAM-DATE-TIME:2026-01-01T00:00:00Z\n\
#EXTINF:2.0,live\n\
seg1.ts\n\
#EXT-X-DATERANGE:ID=\"stitched-ad-1\",CLASS=\"twitch-stitched-ad\",START-DATE=\"2026-01-01T00:00:02Z\",DURATION=4.0\n\
#EXT-X-PROGRAM-DATE-TIME:2026-01-01T00:00:02Z\n\
#EXTINF:2.0,Amazon something\n\
ad1.ts\n\
#EXT-X-PROGRAM-DATE-TIME:2026-01-01T00:00:04Z\n\
#EXTINF:2.0,\n\
ad2.ts\n\
#EXT-X-PROGRAM-DATE-TIME:2026-01-01T00:00:06Z\n\
#EXTINF:2.0,live\n\
seg2.ts\n",
        );

        let mut processor = TwitchPlaylistProcessor::new();
        let processed = processor.process_playlist(&playlist);
        let flags: Vec<bool> = processed.into_iter().map(|p| p.is_ad).collect();
        assert_eq!(flags, vec![false, true, true, false]);
    }

    #[test]
    fn prefetch_after_discontinuity_is_treated_as_ad() {
        let playlist = parse_media_playlist(
            "#EXTM3U\n\
#EXT-X-VERSION:7\n\
#EXT-X-TARGETDURATION:2\n\
#EXT-X-MEDIA-SEQUENCE:1\n\
#EXT-X-PROGRAM-DATE-TIME:2026-01-01T00:00:00Z\n\
#EXTINF:2.0,live\n\
seg1.ts\n\
#EXT-X-DISCONTINUITY\n\
#EXTINF:2.002,PREFETCH_SEGMENT\n\
prefetch1.ts\n",
        );

        let mut processor = TwitchPlaylistProcessor::new();
        let processed = processor.process_playlist(&playlist);

        assert_eq!(processed.len(), 2);
        assert!(!processed[0].is_ad);
        assert!(processed[1].is_ad);
    }

    #[test]
    fn content_after_ad_forces_discontinuity() {
        let playlist = parse_media_playlist(
            "#EXTM3U\n\
#EXT-X-VERSION:7\n\
#EXT-X-TARGETDURATION:2\n\
#EXT-X-MEDIA-SEQUENCE:1\n\
#EXT-X-PROGRAM-DATE-TIME:2026-01-01T00:00:00Z\n\
#EXTINF:2.0,live\n\
seg1.ts\n\
#EXT-X-PROGRAM-DATE-TIME:2026-01-01T00:00:02Z\n\
#EXTINF:2.0,Amazon something\n\
ad1.ts\n\
#EXT-X-PROGRAM-DATE-TIME:2026-01-01T00:00:04Z\n\
#EXTINF:2.0,live\n\
seg2.ts\n",
        );

        let mut processor = TwitchPlaylistProcessor::new();
        let processed = processor.process_playlist(&playlist);

        assert_eq!(processed.len(), 3);
        assert!(processed[1].is_ad);
        assert!(!processed[2].is_ad);
        assert!(processed[2].discontinuity);
    }
}
