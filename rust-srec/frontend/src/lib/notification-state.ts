const NOTIFICATION_LAST_SEEN_CRITICAL_MS_KEY =
  'rust-srec.notifications.lastSeenCriticalMs';

const BROWSER_NOTIFICATIONS_ENABLED_KEY =
  'rust-srec.notifications.browser.enabled';

const BROWSER_NOTIFICATIONS_LAST_NOTIFIED_CRITICAL_MS_KEY =
  'rust-srec.notifications.browser.lastNotifiedCriticalMs';

const NOTIFICATION_STATE_CHANGED_EVENT = 'rust-srec.notifications.stateChanged';

function read(key: string): string | null {
  if (typeof window === 'undefined') return null;
  try {
    return window.localStorage.getItem(key);
  } catch {
    return null;
  }
}

/** A stored timestamp in epoch ms, also accepting a date string; 0 if unset. */
function readMs(key: string): number {
  const raw = read(key);
  if (!raw) return 0;
  const n = Number(raw);
  if (Number.isFinite(n)) return n;
  const parsed = Date.parse(raw);
  return Number.isFinite(parsed) ? parsed : 0;
}

/**
 * Stores `value`, optionally telling `onNotificationStateChanged` listeners in
 * this tab. Unavailable storage only loses the value.
 */
function write(key: string, value: string, announce: boolean): void {
  if (typeof window === 'undefined') return;
  try {
    window.localStorage.setItem(key, value);
    if (announce) {
      window.dispatchEvent(new Event(NOTIFICATION_STATE_CHANGED_EVENT));
    }
  } catch {
    // ignore
  }
}

export function getLastSeenCriticalMs(): number {
  return readMs(NOTIFICATION_LAST_SEEN_CRITICAL_MS_KEY);
}

export function setLastSeenCriticalMs(ms: number): void {
  write(NOTIFICATION_LAST_SEEN_CRITICAL_MS_KEY, String(ms), false);
}

export function getBrowserNotificationsEnabled(): boolean {
  return read(BROWSER_NOTIFICATIONS_ENABLED_KEY) === 'true';
}

export function setBrowserNotificationsEnabled(enabled: boolean): void {
  write(BROWSER_NOTIFICATIONS_ENABLED_KEY, enabled ? 'true' : 'false', true);
}

export function getLastNotifiedCriticalMs(): number {
  return readMs(BROWSER_NOTIFICATIONS_LAST_NOTIFIED_CRITICAL_MS_KEY);
}

export function setLastNotifiedCriticalMs(ms: number): void {
  write(BROWSER_NOTIFICATIONS_LAST_NOTIFIED_CRITICAL_MS_KEY, String(ms), true);
}

export function onNotificationStateChanged(handler: () => void): () => void {
  if (typeof window === 'undefined') return () => undefined;

  const listener = () => handler();
  window.addEventListener(NOTIFICATION_STATE_CHANGED_EVENT, listener);
  window.addEventListener('storage', listener);
  return () => {
    window.removeEventListener(NOTIFICATION_STATE_CHANGED_EVENT, listener);
    window.removeEventListener('storage', listener);
  };
}
