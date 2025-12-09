import { clsx, type ClassValue } from "clsx"
import { twMerge } from "tailwind-merge"

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs))
}

export function getPlatformFromUrl(url: string) {
  if (url.includes('twitch.tv')) return 'twitch';
  if (url.includes('youtube.com') || url.includes('youtu.be')) return 'youtube';
  if (url.includes('douyin.com')) return 'douyin';
  if (url.includes('huya.com')) return 'huya';
  if (url.includes('douyu.com')) return 'douyu';
  if (url.includes('bilibili.com')) return 'bilibili';
  return 'other';
}
