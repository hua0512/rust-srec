import { z } from 'zod';

// --- Filter Schemas ---
export const FilterTypeSchema = z.enum([
  'TIME_BASED',
  'KEYWORD',
  'CATEGORY',
  'CRON',
  'REGEX',
]);

export type FilterType = z.infer<typeof FilterTypeSchema>;

const TimeStringSchema = z
  .string()
  .regex(/^\d{2}:\d{2}(:\d{2})?$/, 'Expected time in HH:MM or HH:MM:SS format')
  .transform((t) => (t.length === 5 ? `${t}:00` : t));

const normalizeTimeToHHMMSS = (value: unknown) => {
  if (typeof value !== 'string') return value;
  if (value.length === 5) return `${value}:00`;
  return value;
};

export const DaysOfWeekSchema = z.enum([
  'Monday',
  'Tuesday',
  'Wednesday',
  'Thursday',
  'Friday',
  'Saturday',
  'Sunday',
]);

// TimeBased filter config (matches backend TimeBasedFilterConfig)
export const TimeBasedFilterConfigSchema = z.object({
  days_of_week: z.array(DaysOfWeekSchema),
  start_time: TimeStringSchema,
  end_time: TimeStringSchema,
});

// Keyword filter config (matches backend KeywordFilterConfig)
export const KeywordFilterConfigSchema = z.object({
  include: z.array(z.string()),
  exclude: z.array(z.string()),
});

// Category filter config (matches backend CategoryFilterConfig)
export const CategoryFilterConfigSchema = z.object({
  categories: z.array(z.string()),
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
  // Keep response parsing permissive to avoid breaking UIs if older/invalid
  // configs already exist in the DB. Validate/normalize per-type in UI.
  config: z.any(),
});

export const CreateFilterRequestSchema = z.discriminatedUnion('filter_type', [
  z.object({
    filter_type: z.literal('TIME_BASED'),
    config: TimeBasedFilterConfigSchema,
  }),
  z.object({
    filter_type: z.literal('KEYWORD'),
    config: KeywordFilterConfigSchema,
  }),
  z.object({
    filter_type: z.literal('CATEGORY'),
    config: CategoryFilterConfigSchema,
  }),
  z.object({
    filter_type: z.literal('CRON'),
    config: CronFilterConfigSchema,
  }),
  z.object({
    filter_type: z.literal('REGEX'),
    config: RegexFilterConfigSchema,
  }),
]);

// Frontend always submits the full filter payload on update, so keep this strict
// (it still matches backend UpdateFilterRequest, which accepts supersets).
export const UpdateFilterRequestSchema = CreateFilterRequestSchema;

const LEGACY_DAY_MAP: Record<string, z.infer<typeof DaysOfWeekSchema>> = {
  Mon: 'Monday',
  Tue: 'Tuesday',
  Wed: 'Wednesday',
  Thu: 'Thursday',
  Fri: 'Friday',
  Sat: 'Saturday',
  Sun: 'Sunday',
};

export function normalizeFilterConfigForType(
  filterType: FilterType,
  config: unknown,
): unknown {
  if (config == null || typeof config !== 'object') return config;
  const c = config as Record<string, unknown>;

  switch (filterType) {
    case 'TIME_BASED': {
      // Legacy UI stored: { days: ["Mon"], start_time: "HH:MM:SS", end_time: "HH:MM:SS" }
      if (Array.isArray(c.days) && !Array.isArray(c.days_of_week)) {
        const days_of_week = c.days
          .map((d) => (typeof d === 'string' ? LEGACY_DAY_MAP[d] : undefined))
          .filter((d): d is z.infer<typeof DaysOfWeekSchema> => !!d);
        return {
          days_of_week,
          start_time: normalizeTimeToHHMMSS(c.start_time),
          end_time: normalizeTimeToHHMMSS(c.end_time),
        };
      }

      return {
        days_of_week: Array.isArray(c.days_of_week) ? c.days_of_week : [],
        start_time: normalizeTimeToHHMMSS(c.start_time),
        end_time: normalizeTimeToHHMMSS(c.end_time),
      };
    }

    case 'KEYWORD': {
      // Legacy UI stored: { keywords: string[], exclude: boolean, case_sensitive: boolean }
      if (
        Array.isArray(c.keywords) &&
        (c.include === undefined || c.exclude === undefined)
      ) {
        const keywords = c.keywords.filter(
          (k): k is string => typeof k === 'string',
        );
        const legacyExclude = c.exclude === true;
        return legacyExclude
          ? { include: [], exclude: keywords }
          : { include: keywords, exclude: [] };
      }

      return {
        include: Array.isArray(c.include) ? c.include : [],
        exclude: Array.isArray(c.exclude) ? c.exclude : [],
      };
    }

    case 'CATEGORY': {
      return {
        categories: Array.isArray(c.categories) ? c.categories : [],
      };
    }

    case 'CRON': {
      return {
        expression:
          typeof c.expression === 'string' ? c.expression : '* * * * * *',
        timezone: typeof c.timezone === 'string' ? c.timezone : undefined,
      };
    }

    case 'REGEX': {
      return {
        pattern: typeof c.pattern === 'string' ? c.pattern : '',
        exclude: c.exclude === true,
        case_insensitive: c.case_insensitive === true,
      };
    }
  }
}
