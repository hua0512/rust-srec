import { useEffect, useMemo, useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { toast } from 'sonner';
import {
  getWebPushPublicKey,
  listWebPushSubscriptions,
  subscribeWebPush,
  unsubscribeWebPush,
} from '@/server/functions/notifications';
import type { WebPushSubscription } from '@/api/schemas';
import { errorMessage } from '@/lib/api-error';
import {
  isWebPushSupported,
  registerWebPushServiceWorker,
  subscribePush,
  subscriptionToJson,
} from '@/lib/web-push';

/**
 * This browser's Web Push subscription: whether it can and does receive
 * pushes, the lowest priority it is sent, and the actions that change them.
 * With `enabled` false (the desktop build, which notifies natively) nothing
 * is probed and the browser reports as unsupported.
 */
export function useWebPushSubscription({ enabled }: { enabled: boolean }) {
  const { i18n } = useLingui();
  const [supported, setSupported] = useState(false);
  const [permission, setPermission] =
    useState<NotificationPermission>('default');
  const [subscribed, setSubscribed] = useState(false);
  const [minPriority, setMinPriority] = useState(10);

  useEffect(() => {
    if (!enabled) return;
    const isSupported = isWebPushSupported();
    setSupported(isSupported);
    if (!isSupported) return;

    setPermission(Notification.permission);

    void (async () => {
      try {
        const reg = await registerWebPushServiceWorker();
        const sub = await reg.pushManager.getSubscription();
        setSubscribed(!!sub);

        if (sub) {
          // If we have a local subscription, sync its state from the backend
          const subs = await listWebPushSubscriptions();
          const serverSub = subs.find(
            (s: WebPushSubscription) => s.endpoint === sub.endpoint,
          );
          if (serverSub) {
            setMinPriority(serverSub.min_priority);
          }
        }
      } catch {
        // ignore
      }
    })();
  }, [enabled]);

  const keyQuery = useQuery({
    queryKey: ['web-push', 'public-key'],
    queryFn: () => getWebPushPublicKey(),
    retry: false,
    enabled: supported,
  });

  const changePriority = async (priority: number) => {
    setMinPriority(priority);

    // If already enabled, update the subscription on the server
    if (subscribed) {
      try {
        const reg = await registerWebPushServiceWorker();
        const sub = await reg.pushManager.getSubscription();
        if (sub) {
          await subscribeWebPush({
            data: {
              subscription: subscriptionToJson(sub),
              min_priority: priority,
            },
          });
          toast.success(i18n._(msg`Web Push priority updated`));
        }
      } catch (e) {
        toast.error(errorMessage(e, i18n._(msg`Failed to update priority`)));
      }
    }
  };

  const toggle = async (checked: boolean) => {
    if (!supported) {
      toast.error(i18n._(msg`Web Push is not supported in this browser`));
      return;
    }

    if (keyQuery.isError) {
      toast.error(i18n._(msg`Web Push is not configured on the server`));
      return;
    }

    if (!checked) {
      try {
        const reg = await registerWebPushServiceWorker();
        const sub = await reg.pushManager.getSubscription();
        if (sub) {
          await unsubscribeWebPush({ data: { endpoint: sub.endpoint } });
          await sub.unsubscribe();
        }
        setSubscribed(false);
        toast.message(i18n._(msg`Web Push disabled for this browser`));
      } catch (e) {
        toast.error(errorMessage(e, i18n._(msg`Failed to disable Web Push`)));
      }
      return;
    }

    // Enabling: permission required.
    let nextPermission: NotificationPermission = Notification.permission;
    if (nextPermission !== 'granted') {
      try {
        nextPermission = await Notification.requestPermission();
      } catch {
        nextPermission = Notification.permission;
      }
    }
    setPermission(nextPermission);
    if (nextPermission !== 'granted') {
      toast.error(i18n._(msg`Notification permission not granted`));
      return;
    }

    try {
      const { public_key } = await getWebPushPublicKey();
      const reg = await registerWebPushServiceWorker();
      let sub = await reg.pushManager.getSubscription();
      if (!sub) {
        sub = await subscribePush(reg, public_key);
      }

      await subscribeWebPush({
        data: {
          subscription: subscriptionToJson(sub),
          min_priority: minPriority,
        },
      });

      setSubscribed(true);
      toast.success(i18n._(msg`Web Push enabled for this browser`));
    } catch (e) {
      toast.error(errorMessage(e, i18n._(msg`Failed to enable Web Push`)));
    }
  };

  const statusText = useMemo(() => {
    if (!supported) return i18n._(msg`Not supported in this browser`);
    if (keyQuery.isError) return i18n._(msg`Server not configured`);
    if (permission === 'denied') return i18n._(msg`Blocked by browser`);
    if (permission === 'granted') return i18n._(msg`Ready`);
    return i18n._(msg`Permission required`);
  }, [i18n, keyQuery.isError, permission, supported]);

  return {
    supported,
    permission,
    subscribed,
    minPriority,
    statusText,
    toggle,
    changePriority,
  };
}
