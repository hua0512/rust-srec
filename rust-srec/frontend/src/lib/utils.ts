import { clsx, type ClassValue } from 'clsx';
import { twMerge } from 'tailwind-merge';

import { BASE_URL } from '../utils/env';
import { getDesktopAccessToken } from '../utils/session';
import { isDesktopBuild } from '../utils/desktop';

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
  if (url.includes('acfun')) return 'acfun';
  return 'other';
}

export function getProxiedUrl(url: string | null | undefined) {
  if (!url) return undefined;

  const desktopBuild = isDesktopBuild();
  const desktopToken = desktopBuild ? getDesktopAccessToken() : null;
  const baseUrl = BASE_URL.endsWith('/') ? BASE_URL.slice(0, -1) : BASE_URL;

  const platform = getPlatformFromUrl(url);

  if (['douyin', 'huya', 'douyu', 'bilibili', 'acfun'].includes(platform)) {
    const headers = {
      Referer: `https://live.${platform !== 'huya' ? platform : 'huya'}.com/`,
    };
    if (platform === 'douyu') {
      headers['Referer'] = 'https://www.douyu.com/';
    }
    if (platform === 'huya') {
      headers['Referer'] = 'https://www.huya.com/';
    }
    if (platform === 'acfun') {
      headers['Referer'] = 'https://live.acfun.cn/';
    }
    const headersParam = encodeURIComponent(JSON.stringify(headers));

    if (desktopBuild) {
      // Desktop builds must proxy via the backend API (no TanStack Start server).
      if (!desktopToken) return url;

      return `${baseUrl}/stream-proxy?url=${encodeURIComponent(url)}&headers=${headersParam}&token=${encodeURIComponent(desktopToken)}`;
    }

    return `/stream-proxy?url=${encodeURIComponent(url)}&headers=${headersParam}`;
  }

  return url;
}

export { formatBytes, formatDuration } from './format';
