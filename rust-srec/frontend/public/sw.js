/* Web Push Service Worker for rust-srec */

self.addEventListener('install', () => {
  self.skipWaiting();
});

self.addEventListener('activate', (event) => {
  event.waitUntil(self.clients.claim());
});

self.addEventListener('push', (event) => {
  let data = {};
  try {
    data = event.data ? event.data.json() : {};
  } catch {
    try {
      data = { body: event.data ? event.data.text() : '' };
    } catch {
      data = {};
    }
  }

  const title = data.title || 'rust-srec';
  const body = data.body || '';
  const url = data.url || '/notifications/events';

  const tag = data.event_log_id ? `rust-srec:${data.event_log_id}` : undefined;

  event.waitUntil(
    self.registration.showNotification(title, {
      body,
      tag,
      data: { url },
      requireInteraction: true,
    }),
  );
});

self.addEventListener('notificationclick', (event) => {
  const url =
    (event.notification &&
      event.notification.data &&
      event.notification.data.url) ||
    '/';
  event.notification.close();

  event.waitUntil(
    (async () => {
      const clientList = await self.clients.matchAll({
        type: 'window',
        includeUncontrolled: true,
      });

      for (const client of clientList) {
        if ('focus' in client) {
          await client.focus();
          if ('navigate' in client) {
            await client.navigate(url);
          }
          return;
        }
      }

      if (self.clients.openWindow) {
        await self.clients.openWindow(url);
      }
    })(),
  );
});
