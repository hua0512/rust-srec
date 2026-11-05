import Hls from 'hls.js';
import { hlsPlaybackConfig, mpegtsPlaybackConfig } from '../playback-presets';

it.each(['low-latency', 'balanced', 'smooth'] as const)(
  'keeps %s compatible with HLS configuration constraints',
  (preset) => {
    const player = new Hls(hlsPlaybackConfig(true, preset));
    expect(player.config.liveMaxLatencyDurationCount).toBeGreaterThan(
      player.config.liveSyncDurationCount,
    );
    player.destroy();
  },
);

it('gives smooth HLS playback more buffer and a later live start than low latency', () => {
  const low = hlsPlaybackConfig(true, 'low-latency');
  const smooth = hlsPlaybackConfig(true, 'smooth');
  expect(smooth.maxBufferLength).toBeGreaterThan(low.maxBufferLength!);
  expect(smooth.liveSyncDurationCount).toBeGreaterThan(
    low.liveSyncDurationCount!,
  );
});

it.each(['low-latency', 'balanced', 'smooth'] as const)(
  'never introduces live tuning for %s recordings',
  (preset) => {
    expect(hlsPlaybackConfig(false, preset)).toEqual({
      enableWorker: true,
      lowLatencyMode: false,
    });
    expect(mpegtsPlaybackConfig(false, preset)).toEqual({});
  },
);

it('keeps MPEG-TS latency chasing bounded', () => {
  const low = mpegtsPlaybackConfig(true, 'low-latency');
  expect(low.liveBufferLatencyMaxLatency).toBeGreaterThan(
    low.liveBufferLatencyMinRemain!,
  );
  expect(low.liveBufferLatencyMinRemain).toBeGreaterThan(0);
  expect(mpegtsPlaybackConfig(true, 'smooth').enableStashBuffer).toBe(true);
});
