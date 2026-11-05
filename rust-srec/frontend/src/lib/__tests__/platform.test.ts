import { formatPlatformName } from '../platform';

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
