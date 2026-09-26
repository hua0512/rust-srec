import { describe, expect, it } from 'vitest';
import {
  AllPlatformConfigsSchema,
  DouyinConfigSchema,
  DouyuConfigSchema,
} from '../platform-configs';

describe('Douyin platform options', () => {
  // These options double as an override layer on a streamer or template, where an absent key
  // means "inherit the platform row". Parsing must not turn an unset option into an override.
  it('keeps an option the caller did not set absent', () => {
    expect(DouyinConfigSchema.parse({ ttwid: 'x' })).toEqual({ ttwid: 'x' });
  });

  it('keeps an option absent through the platform union', () => {
    expect(AllPlatformConfigsSchema.parse({ ttwid: 'x' })).toEqual({
      ttwid: 'x',
    });
  });

  it('preserves an option the caller set to false', () => {
    expect(DouyinConfigSchema.parse({ double_screen: false })).toEqual({
      double_screen: false,
    });
  });
});

describe('Douyu platform options', () => {
  it('preserves device options and validates their values', () => {
    const options = {
      api_mode: 'app',
      device_name: 'OnePlus 12',
      os_version: '15',
      device_id_mode: 'server',
      device_id: '0123456789abcdef0123456789abcdef',
    };
    expect(AllPlatformConfigsSchema.parse(options)).toEqual(options);
    for (const options of [
      { device_id_mode: 'invalid' },
      { device_id: 'bad' },
      { os_version: 14 },
    ]) {
      expect(DouyuConfigSchema.safeParse(options).success).toBe(false);
    }
  });
  it('preserves playback options through the platform union', () => {
    const options = { api_mode: 'app', codec: 'hevc', cdn: 'hw', rate: 0 };
    expect(AllPlatformConfigsSchema.parse(options)).toEqual(options);
    expect(DouyuConfigSchema.parse({})).toEqual({});
    expect(DouyuConfigSchema.parse({ api_mode: null, codec: null })).toEqual({
      api_mode: null,
      codec: null,
    });
  });

  it('rejects unknown playback options and invalid rates', () => {
    for (const options of [
      { api_mode: 'android' },
      { codec: 'av1' },
      { rate: -1 },
      { rate: 1.5 },
    ]) {
      expect(DouyuConfigSchema.safeParse(options).success).toBe(false);
    }
  });
});
