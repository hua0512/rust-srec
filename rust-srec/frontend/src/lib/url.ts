import { BASE_URL } from '@/utils/env';

/**
 * Constructs a full URL for a media resource, handling base URLs and authentication tokens.
 *
 * @param path The relative path or full URL of the media resource.
 * @param token The authentication token to append as a query parameter.
 * @returns The fully constructed URL, or null if the path is invalid.
 */
export function getMediaUrl(path: string | null | undefined, token?: string): string | null {
    if (!path) {
        return null;
    }

    // If it's already a full URL, use it as is
    if (path.startsWith('http')) {
        return path;
    }

    const baseUrl = BASE_URL.endsWith('/') ? BASE_URL.slice(0, -1) : BASE_URL;
    let fullUrl = '';

    // Remove trailing /api if present in base URL because path likely starts with /api (from backend)
    // Backend often returns /api/media/...
    // BASE_URL is likely http://.../api
    const apiBase = baseUrl.endsWith('/api')
        ? baseUrl.slice(0, -4)
        : baseUrl;

    if (path.startsWith('/')) {
        fullUrl = `${apiBase}${path}`;
    } else {
        fullUrl = `${apiBase}/${path}`;
    }


    if (token) {
        const separator = fullUrl.includes('?') ? '&' : '?';
        fullUrl += `${separator}token=${token}`;
    }

    return fullUrl;
}
