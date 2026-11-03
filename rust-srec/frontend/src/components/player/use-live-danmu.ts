import { useRef } from 'react';
import { useQuery } from '@tanstack/react-query';
import { sessionQueryOptions } from '@/api/session';
import { useAuthedWebSocket } from '@/hooks/use-authed-websocket';
import type { LiveDanmuItem } from './danmu-overlay';

/** The handle is unknown or expired; only a renewed one can connect. */
const CLOSE_UNKNOWN_HANDLE = 4404;
/** The platform has no chat to relay; retrying cannot help. */
const CLOSE_UNSUPPORTED = 4422;
/** Too many sockets are open; one may free up, so retry, but slowly. */
const CLOSE_TOO_MANY = 4429;
const TOO_MANY_MIN_DELAY_MS = 15_000;
/**
 * At most one renewal is asked for in this span, so a source whose renewed
 * handles keep failing does not loop.
 */
const RENEWAL_INTERVAL_MS = 60_000;

function isItem(value: unknown): value is LiveDanmuItem {
  if (!value || typeof value !== 'object') return false;
  const item = value as Record<string, unknown>;
  return (
    typeof item.text === 'string' &&
    (item.color === null || typeof item.color === 'number') &&
    (item.kind === 'chat' || item.kind === 'superchat') &&
    typeof item.user === 'string'
  );
}

/** The comments in a `batch` frame; nothing for any other or malformed frame. */
function parseLiveDanmuFrame(data: unknown): LiveDanmuItem[] {
  if (typeof data !== 'string') return [];
  let frame: unknown;
  try {
    frame = JSON.parse(data);
  } catch {
    return [];
  }
  if (!frame || typeof frame !== 'object') return [];
  const { type, items } = frame as Record<string, unknown>;
  if (type !== 'batch' || !Array.isArray(items)) return [];
  return items.filter(isItem);
}

/**
 * Streams a live source's chat to `onItems` while mounted. Unexpected drops
 * are retried with backoff; a new `handle` opens a new socket. When the
 * server no longer knows the handle, `onExpired` is asked for a renewed one.
 */
export function useLiveDanmu(
  handle: string,
  onItems: (items: LiveDanmuItem[]) => void,
  onExpired?: () => void,
): void {
  const lastRenewal = useRef(-Infinity);
  const { data: session } = useQuery({
    ...sessionQueryOptions,
    enabled: typeof window !== 'undefined',
  });
  const token = session?.token?.access_token;

  useAuthedWebSocket({
    accessToken: token,
    path: `/parse/danmu/ws?handle=${encodeURIComponent(handle)}`,
    debugLabel: 'DANMU WS',
    onMessage: (event) => {
      const items = parseLiveDanmuFrame(event.data);
      if (items.length > 0) onItems(items);
    },
    reconnectDelay: (event, delay) => {
      if (event.code === CLOSE_UNKNOWN_HANDLE) {
        const now = Date.now();
        if (onExpired && now - lastRenewal.current >= RENEWAL_INTERVAL_MS) {
          lastRenewal.current = now;
          onExpired();
        }
        return null;
      }
      if (event.code === CLOSE_UNSUPPORTED) return null;
      if (event.code === CLOSE_TOO_MANY) {
        return Math.max(delay, TOO_MANY_MIN_DELAY_MS);
      }
      return delay;
    },
  });
}
