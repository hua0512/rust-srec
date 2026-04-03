import { z } from 'zod';

// Huya platform-specific configuration
export const HuyaPlatformValues = [
  'huya_pc_exe',
  'huya_adr',
  'huya_ios',
  'tv_huya_nftv',
  'huya_webh5',
  'tars_mp',
  'tars_mobile',
  'huya_liveshareh5',
  'random',
] as const;

export const HuyaConfigSchema = z
  .object({
    api_mode: z.enum(['WUP', 'MP', 'WEB']).nullable().optional(),
    platform: z.enum(HuyaPlatformValues).nullable().optional(),
    force_origin_quality: z.boolean().nullable().optional(),
    end_stream_on_danmu_stream_closed: z.boolean().nullable().optional(),
  })
  .strict();

// Douyin platform-specific configuration
export const DouyinConfigSchema = z
  .object({
    force_origin_quality: z.boolean().default(false).nullable().optional(),
    double_screen: z.boolean().default(true).nullable().optional(),
    ttwid_management_mode: z.string().default('global').nullable().optional(),
    ttwid: z.string().nullable().optional(),
    force_mobile_api: z.boolean().default(false).nullable().optional(),
    skip_interactive_games: z.boolean().default(true).nullable().optional(),
    end_stream_on_danmu_stream_closed: z.boolean().nullable().optional(),
  })
  .strict();

// Bilibili platform-specific configuration
export const BilibiliConfigSchema = z
  .object({
    quality: z.number().nullable().optional(),
    end_stream_on_danmu_stream_closed: z.boolean().nullable().optional(),
  })
  .strict();

// Douyu platform-specific configuration
export const DouyuConfigSchema = z
  .object({
    cdn: z.string().nullable().optional(),
    disable_interactive_game: z.boolean().nullable().optional(),
    rate: z.number().nullable().optional(),
    request_retries: z.number().nullable().optional(),
    end_stream_on_danmu_stream_closed: z.boolean().nullable().optional(),
  })
  .strict();

// Twitch platform-specific configuration
export const TwitchConfigSchema = z
  .object({
    oauth_token: z.string().nullable().optional(),
    end_stream_on_danmu_stream_closed: z.boolean().nullable().optional(),
  })
  .strict();

// TikTok platform-specific configuration
export const TikTokConfigSchema = z
  .object({
    force_origin_quality: z.boolean().nullable().optional(),
    end_stream_on_danmu_stream_closed: z.boolean().nullable().optional(),
  })
  .strict();

// Twitcasting platform-specific configuration
export const TwitcastingConfigSchema = z
  .object({
    password: z.string().nullable().optional(),
    end_stream_on_danmu_stream_closed: z.boolean().nullable().optional(),
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
