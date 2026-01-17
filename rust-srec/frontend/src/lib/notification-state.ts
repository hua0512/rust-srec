export const NOTIFICATION_LAST_SEEN_CRITICAL_MS_KEY =
  'rust-srec.notifications.lastSeenCriticalMs';

export const BROWSER_NOTIFICATIONS_ENABLED_KEY =
  'rust-srec.notifications.browser.enabled';

export const BROWSER_NOTIFICATIONS_LAST_NOTIFIED_CRITICAL_MS_KEY =
  'rust-srec.notifications.browser.lastNotifiedCriticalMs';

const NOTIFICATION_STATE_CHANGED_EVENT = 'rust-srec.notifications.stateChanged';

export function getLastSeenCriticalMs(): number {
  if (typeof window === 'undefined') return 0;
  try {
    const raw = window.localStorage.getItem(
      NOTIFICATION_LAST_SEEN_CRITICAL_MS_KEY,
    );
    if (!raw) return 0;
    const n = Number(raw);
    if (Number.isFinite(n)) return n;
    const parsed = Date.parse(raw);
    return Number.isFinite(parsed) ? parsed : 0;
  } catch {
    return 0;
  }
}

export function setLastSeenCriticalMs(ms: number): void {
  if (typeof window === 'undefined') return;
  try {
    window.localStorage.setItem(
      NOTIFICATION_LAST_SEEN_CRITICAL_MS_KEY,
      String(ms),
    );
  } catch {
    // ignore
  }
}

export function getBrowserNotificationsEnabled(): boolean {
  if (typeof window === 'undefined') return false;
  try {
    return (
      window.localStorage.getItem(BROWSER_NOTIFICATIONS_ENABLED_KEY) === 'true'
    );
  } catch {
    return false;
  }
}

export function setBrowserNotificationsEnabled(enabled: boolean): void {
  if (typeof window === 'undefined') return;
  try {
    window.localStorage.setItem(
      BROWSER_NOTIFICATIONS_ENABLED_KEY,
      enabled ? 'true' : 'false',
    );
    window.dispatchEvent(new Event(NOTIFICATION_STATE_CHANGED_EVENT));
  } catch {
    // ignore
  }
}

export function getLastNotifiedCriticalMs(): number {
  if (typeof window === 'undefined') return 0;
  try {
    const raw = window.localStorage.getItem(
      BROWSER_NOTIFICATIONS_LAST_NOTIFIED_CRITICAL_MS_KEY,
    );
    if (!raw) return 0;
    const n = Number(raw);
    if (Number.isFinite(n)) return n;
    const parsed = Date.parse(raw);
    return Number.isFinite(parsed) ? parsed : 0;
  } catch {
    return 0;
  }
}

export function setLastNotifiedCriticalMs(ms: number): void {
  if (typeof window === 'undefined') return;
  try {
    window.localStorage.setItem(
      BROWSER_NOTIFICATIONS_LAST_NOTIFIED_CRITICAL_MS_KEY,
      String(ms),
    );
    window.dispatchEvent(new Event(NOTIFICATION_STATE_CHANGED_EVENT));
  } catch {
    // ignore
  }
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
