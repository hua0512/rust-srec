import { formatBitrate } from '../format';

describe('formatBitrate', () => {
  it('rounds to whole kilobits', () => {
    expect(formatBitrate(2_500_000)).toBe('2500 kbps');
    expect(formatBitrate(1_499)).toBe('1 kbps');
    expect(formatBitrate(1_500)).toBe('2 kbps');
  });

  it('returns undefined without a bitrate', () => {
    expect(formatBitrate(0)).toBeUndefined();
    expect(formatBitrate(null)).toBeUndefined();
    expect(formatBitrate(undefined)).toBeUndefined();
  });
});
