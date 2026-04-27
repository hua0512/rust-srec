import { z } from 'zod';

// --- Lifecycle event types (mirrors `rust-srec/src/session/events.rs`) ---

// `OfflineSignal` from the Rust side — `#[serde(tag = "type", rename_all = "snake_case")]`.
// Variants carry no extra fields here on the wire; richer variants like
// `PlaylistGone(u16)` and `ConsecutiveFailures(u32)` serialise as
// tuple-newtype JSON ({"type": "playlist_gone"} with the integer in a sibling
// field), but on the frontend we only need the discriminator to render a
// human-readable label.
export const OfflineSignalSchema = z.object({
  type: z.string(),
});
export type OfflineSignal = z.infer<typeof OfflineSignalSchema>;

// `TerminalCauseDto` — `#[serde(tag = "type", rename_all = "snake_case")]`.
// Discriminated union so the rendering switch on `cause.type` is exhaustive.
export const TerminalCauseDtoSchema = z.discriminatedUnion('type', [
  z.object({ type: z.literal('completed') }),
  z.object({ type: z.literal('failed'), kind: z.string() }),
  z.object({ type: z.literal('cancelled'), cause: z.string() }),
  z.object({ type: z.literal('rejected'), reason: z.string() }),
  z.object({ type: z.literal('streamer_offline') }),
  z.object({
    type: z.literal('definitive_offline'),
    signal: OfflineSignalSchema,
  }),
]);
export type TerminalCauseDto = z.infer<typeof TerminalCauseDtoSchema>;

// `SessionEventPayload` — `#[serde(tag = "kind", rename_all = "snake_case")]`.
// The discriminator matches the top-level `kind` field on the row, so a
// well-formed row always has `event.kind === event.payload.kind`.
export const SessionEventPayloadSchema = z.discriminatedUnion('kind', [
  z.object({
    kind: z.literal('session_started'),
    from_hysteresis: z.boolean(),
    title: z.string().nullable().optional(),
  }),
  z.object({
    kind: z.literal('hysteresis_entered'),
    cause: TerminalCauseDtoSchema,
    resume_deadline: z.string(),
  }),
  z.object({
    kind: z.literal('session_resumed'),
    hysteresis_duration_secs: z.number(),
  }),
  z.object({
    kind: z.literal('session_ended'),
    cause: TerminalCauseDtoSchema,
    via_hysteresis: z.boolean(),
  }),
]);
export type SessionEventPayload = z.infer<typeof SessionEventPayloadSchema>;

// One row from `session_events`, exposed via `SessionResponse.events`.
// Payload is best-effort: the backend returns `None` if the JSON didn't
// parse, and we accept that here (frontend renders the kind label only).
export const SessionEventSchema = z.object({
  kind: z.string(),
  occurred_at: z.string(),
  payload: SessionEventPayloadSchema.nullable().optional(),
});
export type SessionEvent = z.infer<typeof SessionEventSchema>;

// --- Session Schemas ---
export const SessionSchema = z.object({
  id: z.string(),
  streamer_id: z.string(),
  streamer_name: z.string(),
  streamer_avatar: z.string().nullable().optional(),
  titles: z.array(
    z.object({
      title: z.string(),
      timestamp: z.string(),
    }),
  ),
  // Lifecycle audit log; `default([])` keeps the schema backwards-compatible
  // with sessions that ended before the migration ran.
  events: z.array(SessionEventSchema).default([]),
  title: z.string(),
  start_time: z.string(),
  end_time: z.string().nullable().optional(),
  duration_secs: z.number().nullable().optional(),
  output_count: z.number(),
  total_size_bytes: z.number(),
  danmu_count: z.number().nullable().optional(),
  thumbnail_url: z.string().nullable().optional(),
});

export const DanmuRatePointSchema = z.object({
  ts: z.number(),
  count: z.number(),
});

export const DanmuTopTalkerSchema = z.object({
  user_id: z.string(),
  username: z.string(),
  message_count: z.number(),
});

export const DanmuWordFrequencySchema = z.object({
  word: z.string(),
  count: z.number(),
});

export const SessionDanmuStatisticsSchema = z.object({
  session_id: z.string(),
  total_danmus: z.number(),
  danmu_rate_timeseries: z.array(DanmuRatePointSchema),
  top_talkers: z.array(DanmuTopTalkerSchema),
  word_frequency: z.array(DanmuWordFrequencySchema),
});

export const SessionSegmentSchema = z.object({
  id: z.string(),
  session_id: z.string(),
  segment_index: z.number(),
  file_path: z.string(),
  duration_secs: z.number(),
  size_bytes: z.number(),
  split_reason_code: z.string().nullable().optional(),
  split_reason_details: z.any().optional(),
  created_at: z.string().nullable(),
  completed_at: z.string().nullable().optional(),
  persisted_at: z.string(),
});
export type SessionSegment = z.infer<typeof SessionSegmentSchema>;

export const JobProgressKindSchema = z.enum(['ffmpeg', 'rclone']);
export type JobProgressKind = z.infer<typeof JobProgressKindSchema>;

export type SessionDanmuStatistics = z.infer<
  typeof SessionDanmuStatisticsSchema
>;

export const JobProgressSnapshotSchema = z.object({
  kind: JobProgressKindSchema,
  updated_at: z.string(),
  percent: z.number().nullable().optional(),
  bytes_done: z.number().nullable().optional(),
  bytes_total: z.number().nullable().optional(),
  speed_bytes_per_sec: z.number().nullable().optional(),
  eta_secs: z.number().nullable().optional(),
  out_time_ms: z.number().nullable().optional(),
  raw: z.any().optional(),
});
export type JobProgressSnapshot = z.infer<typeof JobProgressSnapshotSchema>;
