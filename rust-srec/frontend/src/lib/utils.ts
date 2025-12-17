import { clsx, type ClassValue } from 'clsx';
import { twMerge } from 'tailwind-merge';

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

export function getPlatformFromUrl(url: string) {
  if (url.includes('twitch.tv')) return 'twitch';
  if (url.includes('youtube.com') || url.includes('youtu.be')) return 'youtube';
  if (url.includes('douyin')) return 'douyin';
  if (url.includes('huya')) return 'huya';
  if (url.includes('douyu')) return 'douyu';
  if (url.includes('bilibili') || url.includes('hdslb.com')) return 'bilibili';
  return 'other';
}

export function getProxiedUrl(url: string | null | undefined) {
  if (!url) return undefined;

  const platform = getPlatformFromUrl(url);

  if (['douyin', 'huya', 'douyu', 'bilibili'].includes(platform)) {
    const headers = {
      Referer: `https://live.${platform !== 'huya' ? platform : 'huya'}.com/`,
    };
    if (platform === 'douyu') {
      headers['Referer'] = 'https://www.douyu.com/';
    }
    if (platform === 'huya') {
      headers['Referer'] = 'https://www.huya.com/';
    }

    return `/stream-proxy?url=${encodeURIComponent(url)}&headers=${encodeURIComponent(
      JSON.stringify(headers),
    )}`;
  }

  return url;
}

export { formatBytes, formatDuration } from './format';
