import { msg } from '@lingui/core/macro';

export type ConnectionMode = 'auto' | 'direct' | 'proxy';
export type PlaybackStatus =
  | 'resolving'
  | 'connecting'
  | 'ready'
  | 'playing'
  | 'paused'
  | 'buffering'
  | 'ended'
  | 'error';

export const playbackStatusMessages = {
  resolving: msg`Resolving stream`,
  connecting: msg`Connecting`,
  ready: msg`Ready to play`,
  playing: msg`Playing`,
  paused: msg`Paused`,
  buffering: msg`Buffering`,
  ended: msg`Ended`,
  error: msg`Playback error`,
} satisfies Record<PlaybackStatus, unknown>;

export const playbackErrorMessages = {
  authentication: msg`Access was denied. Refresh the stream URL or check the source credentials. If your app session expired, sign in again.`,
  unavailable: msg`The stream is no longer available at this address. Refresh the stream URL or choose another source.`,
  rejected: msg`The server rejected this source. Check the URL and, for local sources, the private network access setting.`,
  upstream: msg`The streaming source could not be reached through the server. Retry or choose another source.`,
  network: msg`The stream could not be loaded. Check the connection, retry, or try the server proxy when using Direct.`,
  media: msg`The browser could not decode this stream. Choose another quality or format.`,
  unsupported: msg`This media format is not supported by your browser. Choose another format or browser.`,
  resolution: msg`The stream URL could not be refreshed. Retry or check the source credentials.`,
  headers: msg`This source needs custom request headers. Use Auto or Server proxy to keep those headers.`,
  session: msg`Sign in to the server before using the stream proxy.`,
  stalled: msg`Playback stalled while seeking. Retry to reload the video.`,
  unknown: msg`Playback could not start. Retry, refresh the stream URL, or choose another source.`,
};

export type PlaybackError = keyof typeof playbackErrorMessages;

export function classifyPlaybackError(
  type: string,
  status?: number,
  proxied = false,
): PlaybackError {
  if (status === 401 || status === 403) return 'authentication';
  if (status === 404 || status === 410) return 'unavailable';
  if (status === 400 && proxied) return 'rejected';
  if (status != null && status >= 500) return proxied ? 'upstream' : 'network';
  if (/unsupported|notsupported/i.test(type)) return 'unsupported';
  if (/media|decode|format/i.test(type)) return 'media';
  if (/network|timeout|eof/i.test(type)) return 'network';
  return 'unknown';
}

export function effectiveConnection(
  mode: ConnectionMode,
  headers?: Record<string, string>,
) {
  return mode === 'proxy' ||
    (mode === 'auto' && Object.keys(headers ?? {}).length > 0)
    ? 'proxy'
    : 'direct';
}

export class PlaybackConfigurationError extends Error {
  constructor(readonly reason: PlaybackError) {
    super(reason);
  }
}
