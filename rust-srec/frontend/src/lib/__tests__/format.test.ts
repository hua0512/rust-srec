import { formatBitrate, formatPlatformName } from '../format';

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

describe('formatPlatformName', () => {
  it('uses the brand spelling where one exists', () => {
    expect(formatPlatformName('soop')).toBe('SOOP');
    expect(formatPlatformName('tiktok')).toBe('TikTok');
    expect(formatPlatformName('twitcasting')).toBe('TwitCasting');
    expect(formatPlatformName('bigo')).toBe('Bigo Live');
    expect(formatPlatformName('acfun')).toBe('AcFun');
  });

  it('capitalizes other platform names', () => {
    expect(formatPlatformName('bilibili')).toBe('Bilibili');
    expect(formatPlatformName('redbook')).toBe('Redbook');
    expect(formatPlatformName('streamlink')).toBe('Streamlink');
  });

  it('does not treat inherited object keys as brands', () => {
    expect(formatPlatformName('constructor')).toBe('Constructor');
  });
});
