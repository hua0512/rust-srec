
export function formatBytes(bytes: number, decimals = 2): string {
    if (bytes === 0) return '0 B';

    const k = 1024;
    const dm = decimals < 0 ? 0 : decimals;
    const sizes = ['B', 'KB', 'MB', 'GB', 'TB', 'PB', 'EB', 'ZB', 'YB'];

    const i = Math.floor(Math.log(bytes) / Math.log(k));

    return parseFloat((bytes / Math.pow(k, i)).toFixed(dm)) + ' ' + sizes[i];
}

export function formatDuration(seconds: number): string {
    if (seconds === 0) return '0s';

    const days = Math.floor(seconds / (3600 * 24));
    const hours = Math.floor((seconds % (3600 * 24)) / 3600);
    const minutes = Math.floor((seconds % 3600) / 60);
    const remainingSeconds = Math.floor(seconds % 60);

    const parts = [];
    if (days > 0) parts.push(`${days}d`);
    if (hours > 0) parts.push(`${hours}h`);
    if (minutes > 0) parts.push(`${minutes}m`);
    if (remainingSeconds > 0) parts.push(`${remainingSeconds}s`);

    return parts.join(' ');
}

/**
 * Format bytes per second to human-readable speed (e.g., "1.5 MB/s")
 * @param bytesPerSec - Speed in bytes per second
 * @param decimals - Number of decimal places (default: 2)
 * @returns Formatted speed string with unit suffix
 */
export function formatSpeed(bytesPerSec: number, decimals = 2): string {
    if (bytesPerSec === 0) return '0 B/s';

    const k = 1024;
    const dm = decimals < 0 ? 0 : decimals;
    const sizes = ['B/s', 'KB/s', 'MB/s', 'GB/s'];

    const i = Math.floor(Math.log(bytesPerSec) / Math.log(k));
    const unitIndex = Math.min(i, sizes.length - 1);

    return parseFloat((bytesPerSec / Math.pow(k, unitIndex)).toFixed(dm)) + ' ' + sizes[unitIndex];
}
