use std::collections::HashMap;

use m3u8_rs::DateRange;
use m3u8_rs::{MediaPlaylist, MediaSegment};
use tracing::debug;

/// Backstop for playlists where pruning can't run (e.g. missing PDT); Twitch
/// playlists normally carry PDT and are pruned explicitly every refresh.
const MAX_AD_DATERANGES: usize = 256;

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

#[derive(Debug)]
pub(super) struct TwitchPlaylistProcessor {
    ad_dateranges: HashMap<String, AdDateRange>,
    pub discontinuity: bool,
}

#[inline]
fn is_prefetch_segment(segment: &MediaSegment) -> bool {
    segment.title.as_deref() == Some(PREFETCH_SEGMENT_TITLE)
}

/// EXTINF title planted by [`preprocess_twitch_playlist`] so prefetch entries
/// survive the m3u8 parser and can be recognized downstream.
pub(super) const PREFETCH_SEGMENT_TITLE: &str = "PREFETCH_SEGMENT";

/// Transforms Twitch-specific tags into m3u8-rs compatible ones.
///
/// - Keeps `#EXT-X-DATERANGE` tags so stitched ads stay detectable
///   (Streamlink logic in [`TwitchPlaylistProcessor`]).
/// - Rewrites `#EXT-X-TWITCH-PREFETCH` tags into standard segment entries
///   titled [`PREFETCH_SEGMENT_TITLE`], so the parser only ever sees standard
///   playlist syntax. Classification (prefetch priority, ad filtering) happens
///   later in the planner, not here.
pub(super) fn preprocess_twitch_playlist(playlist_content: &str) -> String {
    let mut out = String::with_capacity(playlist_content.len());
    for line in playlist_content.lines() {
        if let Some(prefetch_uri) = line.strip_prefix("#EXT-X-TWITCH-PREFETCH:") {
            debug!("Transformed prefetch tag to segment: {}", prefetch_uri);
            // The duration is not provided. Use a placeholder and let the
            // Twitch processor handle ad detection / time extrapolation.
            out.push_str("#EXTINF:2.002,");
            out.push_str(PREFETCH_SEGMENT_TITLE);
            out.push('\n');
            out.push_str(prefetch_uri);
            out.push('\n');
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
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
                let prev = self.ad_dateranges.insert(daterange.id.clone(), ad_range);
                let is_new_or_changed =
                    prev.is_none_or(|prev| prev.start_ms != start_ms || prev.end_ms != end_ms);

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
            self.ad_dateranges.retain(|_id, dr| dr.end_ms >= min_pdt_ms);
        }
        // Without PDT nothing can be pruned by time; drop the ranges that end
        // earliest, which are the least likely to match a later segment.
        while self.ad_dateranges.len() > MAX_AD_DATERANGES {
            let Some(oldest) = self
                .ad_dateranges
                .iter()
                .min_by_key(|(_id, dr)| dr.end_ms)
                .map(|(id, _)| id.clone())
            else {
                break;
            };
            self.ad_dateranges.remove(&oldest);
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
                    .values()
                    .any(|dr| ms >= dr.start_ms && ms < dr.end_ms)
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
    fn ad_ranges_that_ended_before_the_window_are_pruned() {
        let mut processor = TwitchPlaylistProcessor::new();
        processor.process_playlist(&parse_media_playlist(
            "#EXTM3U\n\
#EXT-X-TARGETDURATION:2\n\
#EXT-X-MEDIA-SEQUENCE:1\n\
#EXT-X-DATERANGE:ID=\"stitched-ad-1\",CLASS=\"twitch-stitched-ad\",START-DATE=\"2026-01-01T00:00:00Z\",DURATION=4.0\n\
#EXT-X-PROGRAM-DATE-TIME:2026-01-01T00:00:00Z\n\
#EXTINF:2.0,\n\
ad1.ts\n",
        ));
        assert_eq!(processor.ad_dateranges.len(), 1);

        // The window now starts after the ad ended.
        let later = parse_media_playlist(
            "#EXTM3U\n\
#EXT-X-TARGETDURATION:2\n\
#EXT-X-MEDIA-SEQUENCE:5\n\
#EXT-X-PROGRAM-DATE-TIME:2026-01-01T00:00:10Z\n\
#EXTINF:2.0,live\n\
seg5.ts\n",
        );
        let processed = processor.process_playlist(&later);

        assert!(processor.ad_dateranges.is_empty());
        assert!(!processed[0].is_ad);
    }

    #[test]
    fn ad_ranges_stay_bounded_without_program_date_time() {
        let mut processor = TwitchPlaylistProcessor::new();
        for i in 0..MAX_AD_DATERANGES + 10 {
            // No PDT on the segment, so time-based pruning cannot run.
            processor.process_playlist(&parse_media_playlist(&format!(
                "#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:{i}\n\
#EXT-X-DATERANGE:ID=\"stitched-ad-{i}\",CLASS=\"twitch-stitched-ad\",START-DATE=\"2026-01-01T00:{:02}:{:02}Z\",DURATION=1.0\n\
#EXTINF:2.0,\nad{i}.ts\n",
                i / 60,
                i % 60
            )));
        }

        assert_eq!(processor.ad_dateranges.len(), MAX_AD_DATERANGES);
        // The earliest-ending ranges were dropped; the newest is kept.
        assert!(!processor.ad_dateranges.contains_key("stitched-ad-0"));
        assert!(
            processor
                .ad_dateranges
                .contains_key(&format!("stitched-ad-{}", MAX_AD_DATERANGES + 9))
        );
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
