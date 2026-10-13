import React from 'react';
import { useFormContext, useWatch } from 'react-hook-form';
import { msg } from '@lingui/core/macro';
import {
  Cable,
  Download,
  FileText,
  Gauge,
  HardDrive,
  KeyRound,
  Layers,
  RefreshCw,
  TriangleAlert,
} from 'lucide-react';
import { MESIO_DASH_DEFAULTS } from '@/api/schemas/engine';
import {
  FieldError,
  type PolicyOption,
  type SettingOption,
} from './setting-fields';
import type { Dependency, FieldSpec, SectionSpec } from './setting-sections';

/*
 * Every Mesio setting the form edits, by protocol and section. Paths are relative to the Mesio
 * configuration object; each field's `fallback` is the engine default.
 */

export interface MesioSectionSpec extends SectionSpec {
  /** Whether the settings only apply while the protocol's fixing is on. */
  requiresFix?: boolean;
}

export type Protocol = 'flv' | 'hls' | 'dash';

export interface ProtocolSpec {
  id: Protocol;
  name: string;
  fix: FieldSpec & { kind: 'switch' };
  /** The settings most recordings need, always shown. */
  essentials: MesioSectionSpec;
  /** The rest, folded away. */
  advanced: readonly MesioSectionSpec[];
}

/**
 * The validation message of the DASH track switches, which may not both be off. The rule spans two
 * fields, so it is checked again whenever either changes.
 */
function DashTrackCheck({ basePath }: { basePath: string }) {
  const { control, trigger } = useFormContext();
  const errorPath = `${basePath}.dash.include_audio`;
  const [video, audio] = useWatch({
    control,
    name: [`${basePath}.dash.include_video`, errorPath],
  }) as unknown[];
  const mounted = React.useRef(false);
  React.useEffect(() => {
    if (!mounted.current) {
      mounted.current = true;
      return;
    }
    void trigger(errorPath);
  }, [video, audio, errorPath, trigger]);
  return <FieldError name={errorPath} />;
}

// --- FLV ---

const SEQUENCE_HEADER_MODE_OPTIONS: readonly SettingOption[] = [
  { value: 'crc32', label: msg`Any header change` },
  { value: 'semantic_signature', label: msg`Codec changes only` },
];

const LOOP_PROTECTION: Dependency = {
  path: 'flv_fix.duplicate_tag_filtering',
  fallback: false,
};

const FLV: ProtocolSpec = {
  id: 'flv',
  name: 'FLV',
  fix: {
    kind: 'switch',
    path: 'fix_flv',
    label: msg`Fix FLV Streams`,
    description: msg`Repair headers, timestamps and tag order, and start a new file when the stream format changes. The FLV repair settings below apply only while this is on.`,
    fallback: true,
  },
  essentials: {
    id: 'flv-repair',
    label: msg`Repair`,
    icon: Layers,
    requiresFix: true,
    fields: [
      {
        kind: 'select',
        path: 'flv_fix.sequence_header_change_mode',
        label: msg`Split Files On`,
        description: msg`Any header change splits whenever the stream header bytes change, which is safe but may split more often than needed. Codec changes only splits when the codec configuration changes.`,
        options: SEQUENCE_HEADER_MODE_OPTIONS,
        fallback: 'crc32',
      },
      {
        kind: 'switch',
        path: 'flv_fix.drop_duplicate_sequence_headers',
        label: msg`Drop Repeated Stream Headers`,
        description: msg`Removes redundant headers that can make players stutter.`,
        fallback: false,
      },
      {
        kind: 'switch',
        path: LOOP_PROTECTION.path,
        label: msg`Loop Protection`,
        description: msg`Removes footage a CDN replays after the source stalls.`,
        fallback: LOOP_PROTECTION.fallback,
      },
    ],
  },
  advanced: [
    {
      id: 'flv-loop-protection',
      label: msg`Loop protection`,
      description: msg`Takes effect while Loop Protection is on.`,
      icon: RefreshCw,
      requiresFix: true,
      requires: LOOP_PROTECTION,
      fields: [
        {
          kind: 'number',
          path: 'flv_fix.duplicate_tag_filter_config.window_capacity_tags',
          label: msg`History Size (packets)`,
          fallback: 8192,
        },
        {
          kind: 'number',
          path: 'flv_fix.duplicate_tag_filter_config.window_capacity_bytes',
          label: msg`History Size (bytes)`,
          description: msg`Whichever history limit is reached first applies. 0 keeps no history.`,
          fallback: 16777216,
        },
        {
          kind: 'switch',
          path: 'flv_fix.duplicate_tag_filter_config.enable_replay_offset_matching',
          label: msg`Offset Consistency Check`,
          description: msg`Also removes replays whose timestamps were shifted.`,
          fallback: false,
        },
        {
          kind: 'number',
          path: 'flv_fix.duplicate_tag_filter_config.replay_backjump_threshold_ms',
          label: msg`Backward Jump Threshold (ms)`,
          description: msg`A backward timestamp jump at least this large is checked for a shifted replay.`,
          fallback: 2000,
          requires: {
            path: 'flv_fix.duplicate_tag_filter_config.enable_replay_offset_matching',
            fallback: false,
          },
        },
      ],
    },
    {
      id: 'flv-download',
      label: msg`Download`,
      icon: Download,
      fields: [
        {
          kind: 'number',
          path: 'buffer_size',
          label: msg`Read Buffer Size (bytes)`,
          description: msg`Memory reserved for reading each FLV recording. Values below 65536 use 65536.`,
          fallback: 65536,
        },
      ],
    },
  ],
};

// --- HLS ---

const GAP_STRATEGY_OPTIONS: readonly PolicyOption[] = [
  {
    type: 'wait_indefinitely',
    label: msg`Wait indefinitely`,
    value: { type: 'wait_indefinitely' },
  },
  {
    type: 'skip_after_count',
    label: msg`Skip after count`,
    value: { type: 'skip_after_count', count: 10 },
    payload: [{ key: 'count', label: msg`Count` }],
  },
  {
    type: 'skip_after_duration',
    label: msg`Skip after duration`,
    value: { type: 'skip_after_duration', duration_ms: 5000 },
    payload: [{ key: 'duration_ms', label: msg`Duration`, unit: 'ms' }],
  },
  {
    type: 'skip_after_both',
    label: msg`Skip after both`,
    value: { type: 'skip_after_both', count: 10, duration_ms: 5000 },
    payload: [
      { key: 'count', label: msg`Count` },
      { key: 'duration_ms', label: msg`Duration`, unit: 'ms' },
    ],
  },
];

const VARIANT_POLICY_OPTIONS: readonly PolicyOption[] = [
  {
    type: 'highest_bitrate',
    label: msg`Highest bitrate`,
    value: { type: 'highest_bitrate' },
  },
  {
    type: 'lowest_bitrate',
    label: msg`Lowest bitrate`,
    value: { type: 'lowest_bitrate' },
  },
  {
    type: 'closest_to_bitrate',
    label: msg`Closest to bitrate`,
    value: { type: 'closest_to_bitrate', target_bitrate: 0 },
    payload: [
      { key: 'target_bitrate', label: msg`Target bitrate`, unit: 'bit/s' },
    ],
  },
  { type: 'audio_only', label: msg`Audio only`, value: { type: 'audio_only' } },
  { type: 'video_only', label: msg`Video only`, value: { type: 'video_only' } },
  {
    type: 'matching_resolution',
    label: msg`Matching resolution`,
    value: { type: 'matching_resolution', width: 1920, height: 1080 },
    payload: [
      { key: 'width', label: msg`Width` },
      { key: 'height', label: msg`Height` },
    ],
    separator: ' × ',
  },
  {
    type: 'custom',
    label: msg`Custom`,
    value: { type: 'custom', value: '' },
    payload: [{ key: 'value', label: msg`Value`, numeric: false }],
  },
];

const HTTP_VERSION_OPTIONS: readonly SettingOption[] = [
  { value: 'auto', label: msg`Auto` },
  { value: 'http2_only', label: msg`HTTP/2 only` },
  { value: 'http1_only', label: msg`HTTP/1.1 only` },
];

const ADAPTIVE_REFRESH: Dependency = {
  path: 'hls.playlist_config.adaptive_refresh_enabled',
  fallback: true,
};

const HLS: ProtocolSpec = {
  id: 'hls',
  name: 'HLS',
  fix: {
    kind: 'switch',
    path: 'fix_hls',
    label: msg`Fix HLS Streams`,
    description: msg`Start a new file when the codec, resolution or initialization data changes, and apply file size and duration limits. The settings below apply either way.`,
    fallback: true,
  },
  essentials: {
    id: 'hls-stream',
    label: msg`Stream and requests`,
    icon: Layers,
    fields: [
      {
        kind: 'policy',
        path: 'hls.playlist_config.variant_selection_policy',
        label: msg`Stream Variant`,
        description: msg`Which version of a multi-variant stream is recorded.`,
        options: VARIANT_POLICY_OPTIONS,
        fallback: { type: 'highest_bitrate' },
      },
      {
        kind: 'text',
        path: 'hls.base.user_agent',
        label: msg`User Agent`,
        fallback: '',
        fallbackLabel: 'Mozilla/5.0…',
      },
      {
        kind: 'pairs',
        path: 'hls.base.params',
        label: msg`Query Parameters`,
        description: msg`Added to every HLS request, for example for signed URLs or CDN routing.`,
        fallback: [],
      },
    ],
  },
  advanced: [
    {
      id: 'hls-segments',
      label: msg`Segment downloads`,
      icon: Download,
      fields: [
        {
          kind: 'number',
          path: 'hls.scheduler_config.download_concurrency',
          label: msg`Download Concurrency`,
          description: msg`Segments downloaded at once.`,
          fallback: 5,
        },
        {
          kind: 'number',
          path: 'hls.fetcher_config.segment_download_timeout_ms',
          label: msg`Segment Timeout (ms)`,
          fallback: 10000,
        },
        {
          kind: 'number',
          path: 'hls.fetcher_config.max_segment_retries',
          label: msg`Segment Retries`,
          fallback: 3,
        },
        {
          kind: 'number',
          path: 'hls.fetcher_config.segment_retry_delay_base_ms',
          label: msg`Retry Delay (ms)`,
          fallback: 500,
        },
        {
          kind: 'number',
          path: 'hls.fetcher_config.max_segment_retry_delay_ms',
          label: msg`Max Retry Delay (ms)`,
          fallback: 10000,
        },
      ],
    },
    {
      id: 'hls-playlist',
      label: msg`Playlist refresh`,
      icon: RefreshCw,
      fields: [
        {
          kind: 'number',
          path: 'hls.playlist_config.initial_playlist_fetch_timeout_ms',
          label: msg`First Fetch Timeout (ms)`,
          fallback: 15000,
        },
        {
          kind: 'number',
          path: 'hls.playlist_config.live_refresh_interval_ms',
          label: msg`Refresh Interval (ms)`,
          fallback: 1000,
        },
        {
          kind: 'number',
          path: 'hls.playlist_config.live_max_refresh_retries',
          label: msg`Refresh Retries`,
          fallback: 5,
        },
        {
          kind: 'number',
          path: 'hls.playlist_config.live_refresh_retry_delay_ms',
          label: msg`Refresh Retry Delay (ms)`,
          fallback: 1000,
        },
        {
          kind: 'switch',
          path: ADAPTIVE_REFRESH.path,
          label: msg`Adaptive Refresh`,
          description: msg`Refresh a live playlist according to its target segment duration.`,
          fallback: ADAPTIVE_REFRESH.fallback,
        },
        {
          kind: 'number',
          path: 'hls.playlist_config.adaptive_refresh_min_interval_ms',
          label: msg`Adaptive Min Interval (ms)`,
          fallback: 500,
          requires: ADAPTIVE_REFRESH,
        },
        {
          kind: 'number',
          path: 'hls.playlist_config.adaptive_refresh_max_interval_ms',
          label: msg`Adaptive Max Interval (ms)`,
          fallback: 3000,
          requires: ADAPTIVE_REFRESH,
        },
      ],
    },
    {
      id: 'hls-gaps',
      label: msg`Gaps and stalls`,
      description: msg`What happens when a segment is missing or the stream stops advancing.`,
      icon: TriangleAlert,
      fields: [
        {
          kind: 'policy',
          path: 'hls.output_config.live_gap_strategy',
          label: msg`Live Gap Strategy`,
          options: GAP_STRATEGY_OPTIONS,
          fallback: { type: 'skip_after_both', count: 10, duration_ms: 5000 },
        },
        {
          kind: 'policy',
          path: 'hls.output_config.vod_gap_strategy',
          label: msg`VOD Gap Strategy`,
          options: GAP_STRATEGY_OPTIONS,
          fallback: { type: 'wait_indefinitely' },
        },
        {
          kind: 'disableable',
          path: 'hls.output_config.live_max_overall_stall_duration_ms',
          label: msg`Max Stall Duration (ms)`,
          description: msg`How long a live stream may deliver no data before the recording fails.`,
          fallback: 60000,
          seed: 60000,
        },
        {
          kind: 'disableable',
          path: 'hls.output_config.vod_segment_timeout_ms',
          label: msg`VOD Segment Timeout (ms)`,
          description: msg`How long to wait for a missing VOD segment before skipping it.`,
          fallback: null,
          seed: 30000,
        },
        {
          kind: 'number',
          path: 'hls.output_config.gap_evaluation_interval_ms',
          label: msg`Gap Check Interval (ms)`,
          fallback: 200,
        },
      ],
    },
    {
      id: 'hls-connection',
      label: msg`HTTP connection`,
      icon: Cable,
      fields: [
        {
          kind: 'number',
          path: 'hls.base.connect_timeout_ms',
          label: msg`Connect Timeout (ms)`,
          fallback: 30000,
        },
        {
          kind: 'number',
          path: 'hls.base.read_timeout_ms',
          label: msg`Read Timeout (ms)`,
          fallback: 30000,
        },
        {
          kind: 'number',
          path: 'hls.base.write_timeout_ms',
          label: msg`Write Timeout (ms)`,
          fallback: 30000,
        },
        {
          kind: 'number',
          path: 'hls.base.timeout_ms',
          label: msg`Request Timeout (ms)`,
          fallback: 0,
          fallbackLabel: msg`0, no timeout`,
        },
        {
          kind: 'select',
          path: 'hls.base.http_version',
          label: msg`HTTP Version`,
          options: HTTP_VERSION_OPTIONS,
          fallback: 'auto',
        },
        {
          kind: 'number',
          path: 'hls.base.http2_keep_alive_interval_ms',
          label: msg`HTTP/2 Keep-Alive (ms)`,
          fallback: 20000,
        },
        {
          kind: 'number',
          path: 'hls.base.pool_max_idle_per_host',
          label: msg`Idle Connections per Host`,
          fallback: 10,
        },
        {
          kind: 'number',
          path: 'hls.base.pool_idle_timeout_ms',
          label: msg`Idle Connection Timeout (ms)`,
          fallback: 30000,
        },
        {
          kind: 'switch',
          path: 'hls.base.follow_redirects',
          label: msg`Follow Redirects`,
          fallback: true,
        },
        {
          kind: 'switch',
          path: 'hls.base.danger_accept_invalid_certs',
          label: msg`Accept Invalid Certificates`,
          fallback: false,
        },
        {
          kind: 'switch',
          path: 'hls.base.force_ipv4',
          label: msg`Force IPv4`,
          fallback: false,
        },
        {
          kind: 'switch',
          path: 'hls.base.force_ipv6',
          label: msg`Force IPv6`,
          fallback: false,
        },
      ],
    },
    {
      id: 'hls-keys',
      label: msg`Encryption keys`,
      description: msg`For encrypted streams only.`,
      icon: KeyRound,
      fields: [
        {
          kind: 'number',
          path: 'hls.fetcher_config.key_download_timeout_ms',
          label: msg`Key Timeout (ms)`,
          fallback: 5000,
        },
        {
          kind: 'number',
          path: 'hls.fetcher_config.max_key_retries',
          label: msg`Key Retries`,
          fallback: 3,
        },
        {
          kind: 'number',
          path: 'hls.fetcher_config.key_retry_delay_base_ms',
          label: msg`Key Retry Delay (ms)`,
          fallback: 200,
        },
        {
          kind: 'number',
          path: 'hls.fetcher_config.max_key_retry_delay_ms',
          label: msg`Max Key Retry Delay (ms)`,
          fallback: 5000,
        },
        {
          kind: 'number',
          path: 'hls.decryption_config.key_cache_ttl_ms',
          label: msg`Key Cache TTL (ms)`,
          fallback: 3600000,
        },
        {
          kind: 'switch',
          path: 'hls.decryption_config.offload_decryption_to_cpu_pool',
          label: msg`Decrypt on Worker Threads`,
          description: msg`Keeps decryption from stalling other downloads.`,
          fallback: true,
        },
      ],
    },
    {
      id: 'hls-buffering',
      label: msg`Buffering`,
      description: msg`How many segments are held while waiting for earlier ones.`,
      icon: HardDrive,
      fields: [
        {
          kind: 'number',
          path: 'hls.output_config.live_reorder_buffer_duration_ms',
          label: msg`Live Reorder Window (ms)`,
          fallback: 30000,
        },
        {
          kind: 'number',
          path: 'hls.output_config.live_reorder_buffer_max_segments',
          label: msg`Live Reorder Window (segments)`,
          fallback: 10,
        },
        {
          kind: 'number',
          path: 'hls.output_config.buffer_limits.max_segments',
          label: msg`Buffer Limit (segments)`,
          fallback: 50,
        },
        {
          kind: 'number',
          path: 'hls.output_config.buffer_limits.max_bytes',
          label: msg`Buffer Limit (bytes)`,
          fallback: 104857600,
        },
        {
          kind: 'number',
          path: 'hls.output_config.max_pending_init_segments',
          label: msg`Max Pending Init Segments`,
          description: msg`0 disables the limit.`,
          fallback: 8,
        },
        {
          kind: 'number',
          path: 'hls.scheduler_config.processed_segment_buffer_multiplier',
          label: msg`Processed Buffer Multiplier`,
          description: msg`Multiplies the download concurrency.`,
          fallback: 4,
        },
      ],
    },
    {
      id: 'hls-internals',
      label: msg`Caching and progress`,
      icon: Gauge,
      fields: [
        {
          kind: 'number',
          path: 'hls.cache_config.playlist_ttl_ms',
          label: msg`Playlist Cache TTL (ms)`,
          fallback: 60000,
        },
        {
          kind: 'number',
          path: 'hls.cache_config.segment_ttl_ms',
          label: msg`Segment Cache TTL (ms)`,
          fallback: 120000,
        },
        {
          kind: 'number',
          path: 'hls.processor_config.processed_segment_ttl_ms',
          label: msg`Processed Segment TTL (ms)`,
          fallback: 60000,
        },
        {
          kind: 'number',
          path: 'hls.cache_config.decryption_key_ttl_ms',
          label: msg`Decryption Key Cache TTL (ms)`,
          fallback: 3600000,
        },
        {
          kind: 'number',
          path: 'hls.fetcher_config.progress_emit_min_bytes',
          label: msg`Progress Update Bytes`,
          description: msg`Downloaded bytes between progress updates. 0 updates on every chunk.`,
          fallback: 262144,
        },
        {
          kind: 'number',
          path: 'hls.fetcher_config.progress_emit_min_interval_ms',
          label: msg`Progress Update Interval (ms)`,
          description: msg`Longest time between progress updates while data arrives. 0 updates on every chunk.`,
          fallback: 100,
        },
        {
          kind: 'switch',
          path: 'hls.output_config.metrics_enabled',
          label: msg`Output Metrics`,
          fallback: true,
        },
      ],
    },
  ],
};

// --- DASH ---

const VIDEO_QUALITY_OPTIONS: readonly PolicyOption[] = [
  {
    type: 'highest',
    label: msg`Highest bandwidth`,
    value: { type: 'highest' },
  },
  { type: 'lowest', label: msg`Lowest bandwidth`, value: { type: 'lowest' } },
  {
    type: 'max_height',
    label: msg`Highest up to a height`,
    value: { type: 'max_height', height: 1080 },
    payload: [
      {
        key: 'height',
        label: msg`Maximum height`,
        unit: 'px',
        description: msg`The smallest picture is used when none fits.`,
      },
    ],
  },
  {
    type: 'closest_bandwidth',
    label: msg`Closest to a bandwidth`,
    value: { type: 'closest_bandwidth', bandwidth: 5000000 },
    payload: [
      { key: 'bandwidth', label: msg`Target bandwidth`, unit: 'bit/s' },
    ],
  },
];

const AUDIO_QUALITY_OPTIONS: readonly PolicyOption[] = [
  {
    type: 'highest',
    label: msg`Highest bandwidth`,
    value: { type: 'highest' },
  },
  { type: 'lowest', label: msg`Lowest bandwidth`, value: { type: 'lowest' } },
  {
    type: 'closest_bandwidth',
    label: msg`Closest to a bandwidth`,
    value: { type: 'closest_bandwidth', bandwidth: 128000 },
    payload: [
      { key: 'bandwidth', label: msg`Target bandwidth`, unit: 'bit/s' },
    ],
  },
];

const LIVE_START_OPTIONS: readonly PolicyOption[] = [
  { type: 'auto', label: msg`Automatic`, value: { type: 'auto' } },
  { type: 'live_edge', label: msg`Live edge`, value: { type: 'live_edge' } },
  {
    type: 'delay',
    label: msg`Fixed delay`,
    value: { type: 'delay', delay_ms: 10000 },
    payload: [
      { key: 'delay_ms', label: msg`Delay behind the live edge`, unit: 'ms' },
    ],
  },
];

const DASH: ProtocolSpec = {
  id: 'dash',
  name: 'DASH',
  fix: {
    kind: 'switch',
    path: 'fix_dash',
    label: msg`Fix DASH Streams`,
    description: msg`Start a new file on incompatible stream changes in the merged output, and apply file size and duration limits. The settings below apply either way.`,
    fallback: true,
  },
  essentials: {
    id: 'dash-tracks',
    label: msg`Tracks`,
    icon: Layers,
    after: DashTrackCheck,
    fields: [
      {
        kind: 'policy',
        path: 'dash.video_quality',
        label: msg`Video Quality`,
        options: VIDEO_QUALITY_OPTIONS,
        fallback: MESIO_DASH_DEFAULTS.video_quality,
      },
      {
        kind: 'policy',
        path: 'dash.audio_quality',
        label: msg`Audio Quality`,
        description: msg`Chosen among the audio in the preferred language.`,
        options: AUDIO_QUALITY_OPTIONS,
        fallback: MESIO_DASH_DEFAULTS.audio_quality,
      },
      {
        kind: 'text',
        path: 'dash.audio_language',
        label: msg`Preferred Audio Language`,
        description: msg`A language tag. Any language is used when none matches.`,
        fallback: '',
        fallbackLabel: msg`Any language`,
        placeholder: msg`e.g. en, pt-BR`,
      },
      {
        kind: 'policy',
        path: 'dash.live_start',
        label: msg`Live Start Position`,
        description: msg`Automatic follows the delay the stream suggests, else three segments behind the live edge. Finished streams are always recorded from the beginning.`,
        options: LIVE_START_OPTIONS,
        fallback: MESIO_DASH_DEFAULTS.live_start,
      },
      {
        kind: 'switch',
        path: 'dash.include_video',
        label: msg`Record Video`,
        fallback: MESIO_DASH_DEFAULTS.include_video,
      },
      {
        kind: 'switch',
        path: 'dash.include_audio',
        label: msg`Record Separate Audio`,
        description: msg`Audio inside the video stream is always kept.`,
        fallback: MESIO_DASH_DEFAULTS.include_audio,
      },
    ],
  },
  advanced: [
    {
      id: 'dash-segments',
      label: msg`Segment downloads`,
      icon: Download,
      fields: [
        {
          kind: 'number',
          path: 'dash.download_concurrency',
          label: msg`Download Concurrency`,
          description: msg`Segments downloaded at once, per track.`,
          fallback: MESIO_DASH_DEFAULTS.download_concurrency,
        },
        {
          kind: 'number',
          path: 'dash.segment_timeout_ms',
          label: msg`Segment Timeout (ms)`,
          fallback: MESIO_DASH_DEFAULTS.segment_timeout_ms,
        },
        {
          kind: 'number',
          path: 'dash.max_segment_retries',
          label: msg`Segment Retries`,
          fallback: MESIO_DASH_DEFAULTS.max_segment_retries,
        },
      ],
    },
    {
      id: 'dash-manifest',
      label: msg`Manifest refresh`,
      icon: FileText,
      fields: [
        {
          kind: 'number',
          path: 'dash.manifest_timeout_ms',
          label: msg`Manifest Timeout (ms)`,
          fallback: MESIO_DASH_DEFAULTS.manifest_timeout_ms,
        },
        {
          kind: 'number',
          path: 'dash.min_refresh_interval_ms',
          label: msg`Minimum Refresh Interval (ms)`,
          description: msg`The manifest's own update period applies when longer.`,
          fallback: MESIO_DASH_DEFAULTS.min_refresh_interval_ms,
        },
        {
          kind: 'number',
          path: 'dash.max_refresh_retries',
          label: msg`Refresh Retries`,
          description: msg`Failed manifest refreshes in a row before the recording fails.`,
          fallback: MESIO_DASH_DEFAULTS.max_refresh_retries,
        },
      ],
    },
  ],
};

export const MESIO_PROTOCOLS: readonly ProtocolSpec[] = [FLV, HLS, DASH];

/** Every field of a protocol, its fixing switch included. */
export function protocolFields(protocol: ProtocolSpec): FieldSpec[] {
  return [
    protocol.fix,
    ...protocol.essentials.fields,
    ...protocol.advanced.flatMap((section) => section.fields),
  ];
}
