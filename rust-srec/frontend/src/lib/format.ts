/**
 * Options for formatting utilities
 */
export interface FormatOptions {
  /** Number of decimal places (default: 2) */
  decimals?: number;
  /** Compact format: "1.5h" instead of "1h 30m" (default: false) */
  compact?: boolean;
  /** Value to display for null/undefined/zero (default varies by function) */
  nullValue?: string;
}

/**
 * Format bytes to human-readable string (e.g., "1.5 GB")
 */
export function formatBytes(
  bytes: number | bigint | null | undefined,
  options: FormatOptions = {}
): string {
  const { decimals = 2, nullValue = '0 B' } = options;

  if (bytes == null || !bytes) return nullValue;

  const k = 1024;
  const dm = decimals < 0 ? 0 : decimals;
  const sizes = ['B', 'KB', 'MB', 'GB', 'TB', 'PB', 'EB', 'ZB', 'YB'];

  const numBytes = Number(bytes);
  const i = Math.floor(Math.log(numBytes) / Math.log(k));
  const unitIndex = Math.min(i, sizes.length - 1);

  return parseFloat((numBytes / Math.pow(k, unitIndex)).toFixed(dm)) + ' ' + sizes[unitIndex];
}

/**
 * Format duration in seconds to human-readable string
 * Supports milliseconds, seconds, minutes, hours, and days
 */
export function formatDuration(
  seconds: number | null | undefined,
  options: FormatOptions = {}
): string {
  const { compact = false, nullValue = '-', decimals = 1 } = options;

  if (seconds == null || seconds === 0) return nullValue;

  // Sub-second: show milliseconds
  if (seconds < 1) {
    return `${Math.round(seconds * 1000)}ms`;
  }

  // Compact mode: show as decimal with largest unit
  if (compact) {
    if (seconds < 60) return `${seconds.toFixed(decimals)}s`;
    if (seconds < 3600) return `${(seconds / 60).toFixed(decimals)}m`;
    if (seconds < 86400) return `${(seconds / 3600).toFixed(decimals)}h`;
    return `${(seconds / 86400).toFixed(decimals)}d`;
  }

  // Verbose mode: show multiple units
  if (seconds < 60) {
    return `${seconds.toFixed(decimals)}s`;
  }

  const days = Math.floor(seconds / 86400);
  const hours = Math.floor((seconds % 86400) / 3600);
  const mins = Math.floor((seconds % 3600) / 60);
  const secs = Math.round(seconds % 60);

  const parts: string[] = [];
  if (days > 0) parts.push(`${days}d`);
  if (hours > 0) parts.push(`${hours}h`);
  if (mins > 0) parts.push(`${mins}m`);
  // Only show seconds if less than 1 hour total
  if (seconds < 3600 && secs > 0) parts.push(`${secs}s`);

  return parts.join(' ') || nullValue;
}

/**
 * Format bytes per second to human-readable speed (e.g., "1.5 MB/s")
 */
export function formatSpeed(
  bytesPerSec: number | null | undefined,
  options: FormatOptions = {}
): string {
  const { nullValue = '0 B/s' } = options;

  if (bytesPerSec == null || bytesPerSec === 0) return nullValue;

  const formatted = formatBytes(bytesPerSec, { ...options, nullValue: '0 B' });
  return formatted + '/s';
}

/**
 * Remove null and undefined values from an object or array recursively
 */
export function removeEmpty(obj: any): any {
  if (Array.isArray(obj)) {
    return obj
      .map((v) => removeEmpty(v))
      .filter((v) => v !== null && v !== undefined);
  }
  if (obj !== null && typeof obj === 'object') {
    return Object.fromEntries(
      Object.entries(obj)
        .map(([k, v]) => [k, removeEmpty(v)])
        .filter(([_, v]) => v !== null && v !== undefined),
    );
  }
  return obj;
}