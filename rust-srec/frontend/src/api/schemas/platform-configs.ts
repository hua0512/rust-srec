import { z } from 'zod';

// Huya platform-specific configuration
export const HuyaConfigSchema = z
  .object({
    use_wup: z.boolean().nullable().optional(),
    use_wup_v2: z.boolean().nullable().optional(),
    force_origin_quality: z.boolean().nullable().optional(),
  })
  .strict();

// Douyin platform-specific configuration
export const DouyinConfigSchema = z
  .object({
    force_origin_quality: z.boolean().nullable().optional(),
    double_screen: z.boolean().nullable().optional(),
    ttwid_management_mode: z.string().nullable().optional(),
    ttwid: z.string().nullable().optional(),
    force_mobile_api: z.boolean().nullable().optional(),
    skip_interactive_games: z.boolean().nullable().optional(),
  })
  .strict();

// Bilibili platform-specific configuration
export const BilibiliConfigSchema = z
  .object({
    quality: z.number().nullable().optional(),
  })
  .strict();

// Douyu platform-specific configuration
export const DouyuConfigSchema = z
  .object({
    cdn: z.string().nullable().optional(),
    disable_interactive_game: z.boolean().nullable().optional(),
    rate: z.number().nullable().optional(),
    force_hs: z.boolean().nullable().optional(),
    request_retries: z.number().nullable().optional(),
  })
  .strict();

// Twitch platform-specific configuration
export const TwitchConfigSchema = z
  .object({
    oauth_token: z.string().nullable().optional(),
  })
  .strict();

// TikTok platform-specific configuration
export const TikTokConfigSchema = z
  .object({
    force_origin_quality: z.boolean().nullable().optional(),
  })
  .strict();

// Twitcasting platform-specific configuration
export const TwitcastingConfigSchema = z
  .object({
    password: z.string().nullable().optional(),
  })
  .strict();

// Union of all platform configs
export const AllPlatformConfigsSchema = z.union([
  HuyaConfigSchema,
  DouyinConfigSchema,
  BilibiliConfigSchema,
  DouyuConfigSchema,
  TwitchConfigSchema,
  TikTokConfigSchema,
  TwitcastingConfigSchema,
  z.record(z.string(), z.any()), // Fallback for other platforms
]);
