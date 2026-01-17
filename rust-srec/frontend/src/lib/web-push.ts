export function isWebPushSupported(): boolean {
  return (
    typeof window !== 'undefined' &&
    'Notification' in window &&
    'serviceWorker' in navigator &&
    'PushManager' in window
  );
}

function urlBase64ToUint8Array(base64Url: string): Uint8Array {
  const padding = '='.repeat((4 - (base64Url.length % 4)) % 4);
  const base64 = (base64Url + padding).replace(/-/g, '+').replace(/_/g, '/');
  const raw = atob(base64);
  const output = new Uint8Array(raw.length);
  for (let i = 0; i < raw.length; i++) output[i] = raw.charCodeAt(i);
  return output;
}

export async function registerWebPushServiceWorker(): Promise<ServiceWorkerRegistration> {
  if (!isWebPushSupported()) {
    throw new Error('Web Push is not supported in this browser');
  }
  return navigator.serviceWorker.register('/sw.js');
}

export async function getExistingPushSubscription(
  registration: ServiceWorkerRegistration,
): Promise<PushSubscription | null> {
  return registration.pushManager.getSubscription();
}

export async function subscribePush(
  registration: ServiceWorkerRegistration,
  vapidPublicKeyB64Url: string,
): Promise<PushSubscription> {
  const applicationServerKey = urlBase64ToUint8Array(vapidPublicKeyB64Url);
  return registration.pushManager.subscribe({
    userVisibleOnly: true,
    applicationServerKey,
  });
}

export async function unsubscribePush(
  subscription: PushSubscription,
): Promise<void> {
  await subscription.unsubscribe();
}

export function subscriptionToJson(subscription: PushSubscription): any {
  return subscription.toJSON();
}
