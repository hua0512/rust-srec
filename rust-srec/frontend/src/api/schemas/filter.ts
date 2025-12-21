import { z } from 'zod';

// --- Filter Schemas ---
export const FilterTypeSchema = z.enum([
  'TIME_BASED',
  'KEYWORD',
  'CATEGORY',
  'CRON',
  'REGEX',
]);

// TimeBased filter config
export const TimeBasedFilterConfigSchema = z.object({
  days: z.array(z.string()), // e.g. ["Mon", "Tue"]
  start_time: z.string(), // "HH:MM:SS"
  end_time: z.string(), // "HH:MM:SS"
});

// Keyword filter config
export const KeywordFilterConfigSchema = z.object({
  keywords: z.array(z.string()),
  exclude: z.boolean().default(false),
  case_sensitive: z.boolean().default(false),
});

// Category filter config
export const CategoryFilterConfigSchema = z.object({
  categories: z.array(z.string()),
  exclude: z.boolean().default(false),
});

// Cron filter config
export const CronFilterConfigSchema = z.object({
  expression: z.string(),
  timezone: z.string().optional(),
});

// Regex filter config
export const RegexFilterConfigSchema = z.object({
  pattern: z.string(),
  exclude: z.boolean().default(false),
  case_insensitive: z.boolean().default(false),
});

export const FilterSchema = z.object({
  id: z.string(),
  streamer_id: z.string(),
  filter_type: FilterTypeSchema,
  config: z.any(), // We will cast this to specific type in UI based on filter_type
});

export const CreateFilterRequestSchema = z.object({
  filter_type: FilterTypeSchema,
  config: z.any(),
});

export const UpdateFilterRequestSchema = z.object({
  filter_type: FilterTypeSchema.optional(),
  config: z.any().optional(),
});
