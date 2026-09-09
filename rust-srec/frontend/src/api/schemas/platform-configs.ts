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

// Douyin platform-specific configuration.
//
// Every field is optional with no schema default: this shape is also an override layer on a
// streamer or template, where an absent key means "inherit" and a present one wins over the
// platform row. A schema default would turn any save into an override of every option.
export const DouyinConfigSchema = z
  .object({
    force_origin_quality: z.boolean().nullable().optional(),
    double_screen: z.boolean().nullable().optional(),
    ttwid_management_mode: z.string().nullable().optional(),
    ttwid: z.string().nullable().optional(),
    force_mobile_api: z.boolean().nullable().optional(),
    skip_interactive_games: z.boolean().nullable().optional(),
    end_stream_on_danmu_stream_closed: z.boolean().nullable().optional(),
  })
  .strict();

/**
 * What the extractor assumes for a Douyin option that no configuration layer sets.
 *
 * Mirrors the extractor's own fallbacks so the platform editor can show the effective value
 * instead of an unset switch. Display only: the override layers leave an unset option absent.
 */
export const DOUYIN_CONFIG_DISPLAY_DEFAULTS = {
  force_origin_quality: false,
  double_screen: true,
  ttwid_management_mode: 'global',
  force_mobile_api: false,
  skip_interactive_games: true,
} as const;

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
    only_audio: z.boolean().nullable().optional(),
    rate: z.number().nullable().optional(),
    request_retries: z.number().int().nullable().optional(),
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

// SOOP platform-specific configuration
export const SoopConfigSchema = z
  .object({
    username: z.string().nullable().optional(),
    password: z.string().nullable().optional(),
    stream_password: z.string().nullable().optional(),
  })
  .strict();

// Bigo Live platform-specific configuration
export const BigoConfigSchema = z
  .object({
    stream_password: z.string().nullable().optional(),
    mint_token: z.boolean().nullable().optional(),
  })
  .strict();

// Streamlink *extractor* configuration.
//
// Distinct from `StreamlinkConfigSchema` in `engine.ts`, which configures the streamlink download
// engine. This one configures stream-URL resolution and is nested under a `streamlink` key in the
// extras blob rather than sitting at the top level like the platform configs above.
export const StreamlinkExtractorConfigSchema = z
  .object({
    binary_path: z.string().nullable().optional(),
    quality: z.string().nullable().optional(),
    extra_args: z.array(z.string()).nullable().optional(),
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
  SoopConfigSchema,
  BigoConfigSchema,
  z.record(z.string(), z.any()), // Fallback for other platforms
]);

/**
 * Extractor selection, mirroring `ExtractorSelection` on the backend.
 *
 * `auto` dispatches on the URL; `streamlink` forces the streamlink CLI even for a URL a built-in
 * platform would otherwise handle. Independent of the download engine.
 */
export const ExtractorSelectionSchema = z.enum(['auto', 'streamlink']);
export type ExtractorSelection = z.infer<typeof ExtractorSelectionSchema>;
