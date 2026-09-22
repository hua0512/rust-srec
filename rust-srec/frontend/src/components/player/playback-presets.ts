import type { HlsConfig } from 'hls.js';
import type Mpegts from 'mpegts.js';
import { msg } from '@lingui/core/macro';

export type PlaybackPreset = 'low-latency' | 'balanced' | 'smooth';

export const playbackPresetMessages = {
  'low-latency': msg`Stay closer to live. Network interruptions may cause more buffering.`,
  balanced: msg`Use the player's default balance of buffering and delay.`,
  smooth: msg`Allow more startup buffering. Playback may start later and add delay.`,
};

export function hlsPlaybackConfig(
  isLive: boolean,
  preset: PlaybackPreset,
): Partial<HlsConfig> {
  const base = { enableWorker: true, lowLatencyMode: isLive };
  if (!isLive || preset === 'balanced') return base;
  // Counts scale with the source's segment duration; these are not promises
  // of a fixed end-to-end delay. Keep maximum latency above the sync target.
  return preset === 'low-latency'
    ? {
        ...base,
        liveSyncDurationCount: 2,
        liveMaxLatencyDurationCount: 4,
        maxBufferLength: 10,
        backBufferLength: 30,
      }
    : {
        ...base,
        lowLatencyMode: false,
        liveSyncDurationCount: 5,
        liveMaxLatencyDurationCount: 10,
        maxBufferLength: 60,
        backBufferLength: 60,
      };
}

export function mpegtsPlaybackConfig(
  isLive: boolean,
  preset: PlaybackPreset,
): Mpegts.Config {
  // Preserve recording seek behavior and the engine defaults for Balanced.
  if (!isLive || preset === 'balanced') return {};
  return preset === 'low-latency'
    ? {
        enableStashBuffer: false,
        liveBufferLatencyChasing: true,
        liveBufferLatencyChasingOnPaused: false,
        liveBufferLatencyMaxLatency: 3,
        liveBufferLatencyMinRemain: 1,
      }
    : {
        enableStashBuffer: true,
        stashInitialSize: 256 * 1024,
        liveBufferLatencyChasing: false,
        liveSync: false,
      };
}
