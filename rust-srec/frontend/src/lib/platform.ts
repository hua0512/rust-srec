/** The platform a streamer or media URL belongs to, or `other`. */
export function getPlatformFromUrl(url: string) {
  if (url.includes('twitch.tv')) return 'twitch';
  if (url.includes('youtube.com') || url.includes('youtu.be')) return 'youtube';
  if (url.includes('douyin')) return 'douyin';
  if (url.includes('huya')) return 'huya';
  if (url.includes('douyu')) return 'douyu';
  if (url.includes('bilibili') || url.includes('hdslb.com')) return 'bilibili';
  if (url.includes('acfun')) return 'acfun';
  if (url.includes('tiktok.com')) return 'tiktok';
  if (url.includes('pandalive.co.kr')) return 'pandatv';
  if (url.includes('picarto.tv')) return 'picarto';
  if (url.includes('xhslink.com')) return 'redbook';
  if (url.includes('twitcasting.tv')) return 'twitcasting';
  if (url.includes('weibo.com')) return 'weibo';
  if (
    url.includes('sooplive.co.kr') ||
    url.includes('sooplive.com') ||
    url.includes('afreecatv.com')
  )
    return 'soop';
  if (url.includes('bigo.tv')) return 'bigo';
  return 'other';
}

/**
 * Brand spellings of the platforms whose name is not just the stored key
 * capitalized. Keys are the lowercase platform names the backend stores.
 */
const PLATFORM_BRAND_NAMES: Readonly<Record<string, string>> = {
  acfun: 'AcFun',
  bigo: 'Bigo Live',
  pandatv: 'PandaTV',
  soop: 'SOOP',
  tiktok: 'TikTok',
  twitcasting: 'TwitCasting',
  youtube: 'YouTube',
};

/**
 * A platform's display name. Platforms are stored by lowercase name
 * (`bilibili`); the UI shows the brand spelling (`SOOP`, `TikTok`), or the
 * name capitalized (`Bilibili`) for platforms without one.
 */
export function formatPlatformName(name: string): string {
  if (Object.hasOwn(PLATFORM_BRAND_NAMES, name))
    return PLATFORM_BRAND_NAMES[name];
  return name.charAt(0).toUpperCase() + name.slice(1);
}
