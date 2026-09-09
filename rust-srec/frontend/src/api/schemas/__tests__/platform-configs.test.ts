import { describe, expect, it } from 'vitest';
import {
  AllPlatformConfigsSchema,
  DouyinConfigSchema,
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
