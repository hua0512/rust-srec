import { describe, expect, it } from 'vitest';

import { StreamerSpecificConfigSchema } from '../streamer';
import { GlobalConfigSchema } from '../system';

describe('danmu statistics config decoding', () => {
  it('parses the global JSON string returned by the backend', () => {
    const result = GlobalConfigSchema.shape.danmu_statistics.parse(
      JSON.stringify({ enabled: false, top_words: 25 }),
    );

    expect(result).toEqual({ enabled: false, top_words: 25 });
  });

  it('accepts the object nested in a streamer response', () => {
    const result = StreamerSpecificConfigSchema.parse({
      danmu_statistics: { enabled: false, top_words: 25 },
    });

    expect(result.danmu_statistics).toEqual({
      enabled: false,
      top_words: 25,
    });
  });
});
