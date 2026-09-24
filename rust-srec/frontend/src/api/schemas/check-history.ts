import { z } from 'zod';

// One row of the streamer's per-poll check history. Mirrors
// `StreamerCheckHistoryEntry` in the Rust API; defensive parsing —
// `stream_selected` and `streams_extracted_detail` are tolerant
// because malformed persisted JSON degrades to `null` server-side and
// we don't want a render to fail on a stray bar.
const SelectedStreamSummarySchema = z.object({
  quality: z.string().optional(),
  stream_format: z.string().optional(),
  media_format: z.string().optional(),
  bitrate: z.number().optional(),
  codec: z.string().optional(),
  fps: z.number().optional(),
});

export const StreamerCheckHistoryEntrySchema = z.object({
  checked_at: z.string(), // ISO datetime
  duration_ms: z.number(),
  outcome: z.enum([
    'live',
    'offline',
    'filtered',
    'transient_error',
    'fatal_error',
  ]),
  fatal_kind: z.string().nullable().optional(),
  filter_reason: z.string().nullable().optional(),
  error_message: z.string().nullable().optional(),
  streams_extracted: z.number(),
  stream_selected: SelectedStreamSummarySchema.nullable().optional(),
  streams_extracted_detail: z
    .array(SelectedStreamSummarySchema)
    .nullable()
    .optional(),
  title: z.string().nullable().optional(),
  category: z.string().nullable().optional(),
  viewer_count: z.number().nullable().optional(),
});

export type StreamerCheckHistoryEntry = z.infer<
  typeof StreamerCheckHistoryEntrySchema
>;
