import { z } from 'zod';

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
