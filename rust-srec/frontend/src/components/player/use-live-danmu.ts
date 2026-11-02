import { useQuery } from '@tanstack/react-query';
import { sessionQueryOptions } from '@/api/session';
import { useAuthedWebSocket } from '@/hooks/use-authed-websocket';
import type { LiveDanmuItem } from './danmu-overlay';

/**
 * Closes the server sends for a handle it will never serve: unknown or
 * expired, a platform without chat, or too many sockets. Retrying the same
 * handle cannot help; a renewed handle opens a new socket.
 */
const FINAL_CLOSE_CODES = new Set([4404, 4422, 4429]);

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
 * are retried with backoff; a new `handle` opens a new socket.
 */
export function useLiveDanmu(
  handle: string,
  onItems: (items: LiveDanmuItem[]) => void,
): void {
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
    shouldReconnect: (event) => !FINAL_CLOSE_CODES.has(event.code),
  });
}
