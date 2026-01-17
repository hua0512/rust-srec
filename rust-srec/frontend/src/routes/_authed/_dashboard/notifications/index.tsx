import { useEffect, useMemo, useState } from 'react';
import { createFileRoute } from '@tanstack/react-router';
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import {
  listChannels,
  deleteChannel,
  testChannel,
  getWebPushPublicKey,
  subscribeWebPush,
  unsubscribeWebPush,
  listWebPushSubscriptions,
} from '@/server/functions/notifications';
import { Button } from '@/components/ui/button';
import { Skeleton } from '@/components/ui/skeleton';
import { Plus, Bell, Settings2, ListOrdered, Monitor } from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { t } from '@lingui/core/macro';
import { toast } from 'sonner';
import { ChannelCard } from '@/components/notifications/channel-card';
import { ChannelForm } from '@/components/notifications/channel-form';
import { SubscriptionManager } from '@/components/notifications/subscription-manager';
import { BrowserChannelDialog } from '@/components/notifications/browser-channel-dialog';
import { NotificationChannel, WebPushSubscription } from '@/api/schemas';
import { motion, AnimatePresence } from 'motion/react';
import { Badge } from '@/components/ui/badge';
import { Link } from '@tanstack/react-router';
import { cn } from '@/lib/utils';
import { useNotificationDot } from '@/hooks/use-notification-dot';
import {
  getBrowserNotificationsEnabled,
  setBrowserNotificationsEnabled,
} from '@/lib/notification-state';
import {
  getExistingPushSubscription,
  isWebPushSupported,
  registerWebPushServiceWorker,
  subscribePush,
  subscriptionToJson,
  unsubscribePush,
} from '@/lib/web-push';

export const Route = createFileRoute('/_authed/_dashboard/notifications/')({
  component: NotificationsPage,
});

function NotificationsPage() {
  const queryClient = useQueryClient();
  const [isFormOpen, setIsFormOpen] = useState(false);
  const [editingChannel, setEditingChannel] =
    useState<NotificationChannel | null>(null);
  const [isSubsOpen, setIsSubsOpen] = useState(false);
  const [subsChannel, setSubsChannel] = useState<NotificationChannel | null>(
    null,
  );

  const [webPushSupported, setWebPushSupported] = useState(false);
  const [webPushPermission, setWebPushPermission] =
    useState<NotificationPermission>('default');
  const [webPushEnabled, setWebPushEnabled] = useState(false);
  const [webPushMinPriority, setWebPushMinPriority] = useState('critical');

  const [isBrowserDialogOpen, setIsBrowserDialogOpen] = useState(false);
  const [browserNotificationsEnabled, setBrowserNotificationsEnabledState] =
    useState(false);

  const { hasCriticalDot } = useNotificationDot();

  useEffect(() => {
    setBrowserNotificationsEnabledState(getBrowserNotificationsEnabled());
  }, []);

  const handleBrowserNotificationsToggle = (enabled: boolean) => {
    setBrowserNotificationsEnabled(enabled);
    setBrowserNotificationsEnabledState(enabled);
    if (enabled) {
      toast.success(t`Live polling notifications enabled`);
    }
  };

  useEffect(() => {
    const supported = isWebPushSupported();
    setWebPushSupported(supported);
    if (!supported) return;

    setWebPushPermission(Notification.permission);

    (async () => {
      try {
        const reg = await registerWebPushServiceWorker();
        const sub = await getExistingPushSubscription(reg);
        setWebPushEnabled(!!sub);

        if (sub) {
          // If we have a local subscription, sync its state from the backend
          const subs = await listWebPushSubscriptions();
          const serverSub = subs.find(
            (s: WebPushSubscription) => s.endpoint === sub.endpoint,
          );
          if (serverSub) {
            setWebPushMinPriority(serverSub.min_priority);
          }
        }
      } catch {
        // ignore
      }
    })();
  }, []);

  const { data: channels, isLoading } = useQuery({
    queryKey: ['notification-channels'],
    queryFn: () => listChannels(),
  });

  const webPushKeyQuery = useQuery({
    queryKey: ['web-push', 'public-key'],
    queryFn: () => getWebPushPublicKey(),
    retry: false,
    enabled: webPushSupported,
  });

  const handleWebPushPriorityChange = async (priority: string) => {
    setWebPushMinPriority(priority);

    // If already enabled, update the subscription on the server
    if (webPushEnabled) {
      try {
        const reg = await registerWebPushServiceWorker();
        const sub = await getExistingPushSubscription(reg);
        if (sub) {
          await subscribeWebPush({
            data: {
              subscription: subscriptionToJson(sub),
              min_priority: priority,
            },
          });
          queryClient.invalidateQueries({
            queryKey: ['web-push', 'subscriptions'],
          });
          toast.success(t`Web Push priority updated`);
        }
      } catch (e: any) {
        toast.error(e?.message || t`Failed to update priority`);
      }
    }
  };

  const toggleWebPush = async (checked: boolean) => {
    if (!webPushSupported) {
      toast.error(t`Web Push is not supported in this browser`);
      return;
    }

    if (webPushKeyQuery.isError) {
      toast.error(t`Web Push is not configured on the server`);
      return;
    }

    if (!checked) {
      try {
        const reg = await registerWebPushServiceWorker();
        const sub = await getExistingPushSubscription(reg);
        if (sub) {
          await unsubscribeWebPush({ data: { endpoint: sub.endpoint } });
          await unsubscribePush(sub);
        }
        setWebPushEnabled(false);
        queryClient.invalidateQueries({
          queryKey: ['web-push', 'subscriptions'],
        });
        toast.message(t`Web Push disabled for this browser`);
      } catch (e: any) {
        toast.error(e?.message || t`Failed to disable Web Push`);
      }
      return;
    }

    // Enabling: permission required.
    let permission: NotificationPermission = Notification.permission;
    if (permission !== 'granted') {
      try {
        permission = await Notification.requestPermission();
      } catch {
        permission = Notification.permission;
      }
    }
    setWebPushPermission(permission);
    if (permission !== 'granted') {
      toast.error(t`Notification permission not granted`);
      return;
    }

    try {
      const { public_key } = await getWebPushPublicKey();
      const reg = await registerWebPushServiceWorker();
      let sub = await getExistingPushSubscription(reg);
      if (!sub) {
        sub = await subscribePush(reg, public_key);
      }

      await subscribeWebPush({
        data: {
          subscription: subscriptionToJson(sub),
          min_priority: webPushMinPriority,
        },
      });

      setWebPushEnabled(true);
      queryClient.invalidateQueries({
        queryKey: ['web-push', 'subscriptions'],
      });
      toast.success(t`Web Push enabled for this browser`);
    } catch (e: any) {
      toast.error(e?.message || t`Failed to enable Web Push`);
    }
  };

  const deleteMutation = useMutation({
    mutationFn: (id: string) => deleteChannel({ data: id }),
    onSuccess: () => {
      toast.success(t`Channel deleted`);
      queryClient.invalidateQueries({ queryKey: ['notification-channels'] });
    },
    onError: (err: any) => {
      toast.error(err.message || t`Failed to delete channel`);
    },
  });

  const testMutation = useMutation({
    mutationFn: (id: string) => testChannel({ data: id }),
    onSuccess: () => {
      toast.success(t`Test notification sent`);
    },
    onError: (err: any) => {
      toast.error(err.message || t`Failed to send test notification`);
    },
  });

  const handleEdit = (channel: NotificationChannel) => {
    setEditingChannel(channel);
    setIsFormOpen(true);
  };

  const handleDelete = (id: string) => {
    deleteMutation.mutate(id);
  };

  const handleTest = (id: string) => {
    testMutation.mutate(id);
  };

  const handleManageSubscriptions = (channel: NotificationChannel) => {
    setSubsChannel(channel);
    setIsSubsOpen(true);
  };

  const handleCreate = () => {
    setEditingChannel(null);
    setIsFormOpen(true);
  };

  const container = {
    hidden: { opacity: 0 },
    show: {
      opacity: 1,
      transition: {
        staggerChildren: 0.1,
      },
    },
  };

  const item = {
    hidden: { opacity: 0, y: 20 },
    show: { opacity: 1, y: 0 },
  };

  const webPushStatusText = useMemo(() => {
    if (!webPushSupported) return t`Not supported in this browser`;
    if (webPushKeyQuery.isError) return t`Server not configured`;
    if (webPushPermission === 'denied') return t`Blocked by browser`;
    if (webPushPermission === 'granted') return t`Ready`;
    return t`Permission required`;
  }, [webPushKeyQuery.isError, webPushPermission, webPushSupported]);

  return (
    <motion.div
      className="min-h-screen space-y-6"
      variants={container}
      initial="hidden"
      animate="show"
    >
      {/* Header */}
      <motion.div className="border-b border-border/40" variants={item}>
        <div className="w-full">
          <div className="flex flex-col md:flex-row gap-4 items-start md:items-center justify-between p-4 md:px-8">
            <div className="flex items-center gap-4">
              <div className="p-2.5 rounded-xl bg-gradient-to-br from-primary/20 to-primary/5 ring-1 ring-primary/10">
                <Bell className="h-6 w-6 text-primary" />
              </div>
              <div>
                <h1 className="text-xl font-semibold tracking-tight">
                  <Trans>Notifications</Trans>
                </h1>
                <p className="text-sm text-muted-foreground">
                  <Trans>Manage where system notifications are sent</Trans>
                </p>
              </div>
            </div>
            <div className="flex items-center gap-2 w-full md:w-auto">
              <Badge variant="secondary" className="h-9 px-3 text-sm">
                {(channels?.length || 0) + 1} <Trans>channels</Trans>
              </Badge>
              <Button variant="outline" asChild className="relative">
                <Link to="/notifications/events">
                  <ListOrdered className="mr-2 h-4 w-4" />
                  <Trans>Events</Trans>
                  {hasCriticalDot && (
                    <div className="absolute -top-1 -right-1 flex items-center justify-center">
                      <motion.div
                        animate={{
                          scale: [1, 1.8, 1],
                          opacity: [0.5, 0, 0.5],
                        }}
                        transition={{
                          duration: 2,
                          repeat: Infinity,
                          ease: 'easeInOut',
                        }}
                        className="absolute h-3 w-3 rounded-full bg-red-500/60 blur-[1px]"
                      />
                      <div className="relative h-2 w-2 rounded-full bg-red-500 ring-2 ring-background shadow-[0_0_10px_rgba(239,68,68,0.6)]" />
                    </div>
                  )}
                </Link>
              </Button>
              <Button onClick={handleCreate}>
                <Plus className="mr-2 h-4 w-4" />
                <Trans>Add Channel</Trans>
              </Button>
            </div>
          </div>
        </div>
      </motion.div>

      <div className="p-4 md:px-8 pb-20 w-full">
        <AnimatePresence mode="wait">
          {isLoading ? (
            <motion.div
              key="loading"
              initial={{ opacity: 0 }}
              animate={{ opacity: 1 }}
              exit={{ opacity: 0 }}
              className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 gap-6"
            >
              {[1, 2, 3, 4].map((i) => (
                <div
                  key={i}
                  className="h-[200px] border rounded-xl bg-muted/10 animate-pulse flex flex-col p-6 space-y-4 shadow-sm"
                >
                  <div className="flex justify-between items-start">
                    <Skeleton className="h-10 w-10 rounded-full" />
                    <Skeleton className="h-6 w-8" />
                  </div>
                  <div className="space-y-2 pt-2">
                    <Skeleton className="h-6 w-3/4" />
                    <Skeleton className="h-4 w-1/2" />
                  </div>
                </div>
              ))}
            </motion.div>
          ) : (
            <motion.div
              key="list"
              variants={container}
              initial="hidden"
              animate="show"
              className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 gap-6"
            >
              {/* Browser Card First */}
              <motion.div variants={item}>
                <div
                  onClick={() => setIsBrowserDialogOpen(true)}
                  className="cursor-pointer relative overflow-hidden group border border-border/40 hover:border-primary/20 transition-all duration-500 hover:shadow-2xl hover:shadow-primary/10 bg-gradient-to-br from-background/80 to-background/40 backdrop-blur-xl rounded-3xl p-6 h-full flex flex-col justify-between min-h-[220px]"
                >
                  <div className="absolute inset-x-0 top-0 h-0.5 bg-gradient-to-r from-transparent via-primary/40 to-transparent opacity-0 group-hover:opacity-100 transition-opacity duration-700" />

                  {/* Hover Glow Effect */}
                  <div className="absolute -inset-0.5 bg-gradient-to-br from-primary/5 to-transparent opacity-0 group-hover:opacity-100 blur-2xl transition-opacity duration-500 pointer-events-none" />

                  <div className="relative z-10">
                    <div className="flex justify-between items-start mb-4">
                      <div className="p-3 rounded-2xl bg-primary/10 ring-1 ring-inset ring-black/5 dark:ring-white/5 transition-transform duration-500 group-hover:scale-110 group-hover:rotate-3">
                        <Monitor className="h-5 w-5 text-primary" />
                      </div>
                      <Badge
                        variant={
                          browserNotificationsEnabled || webPushEnabled
                            ? 'default'
                            : 'secondary'
                        }
                        className={cn(
                          'text-[10px] font-bold uppercase tracking-wider px-2 py-0 h-5 border-none shadow-none',
                          browserNotificationsEnabled || webPushEnabled
                            ? 'bg-primary/20 text-primary hover:bg-primary/30'
                            : 'bg-muted/30 text-muted-foreground',
                        )}
                      >
                        {browserNotificationsEnabled || webPushEnabled
                          ? t`Active`
                          : t`Inactive`}
                      </Badge>
                    </div>
                    <div className="space-y-1">
                      <h3 className="text-base font-medium tracking-tight group-hover:text-primary transition-colors duration-300">
                        <Trans>Browser Notifications</Trans>
                      </h3>
                      <div className="text-[10px] uppercase tracking-wider font-semibold text-muted-foreground/60">
                        <Trans>Internal</Trans>
                      </div>
                    </div>

                    <div className="mt-4 space-y-2">
                      <div className="flex flex-col gap-0.5 bg-muted/30 rounded-md px-2 py-1.5 border border-transparent group-hover:border-primary/5 transition-colors">
                        <span className="text-[9px] uppercase tracking-wider opacity-50">
                          <Trans>Live Polling</Trans>
                        </span>
                        <span className="text-[11px] font-medium text-foreground/80">
                          {browserNotificationsEnabled
                            ? t`Enabled`
                            : t`Disabled`}
                        </span>
                      </div>
                      <div className="flex flex-col gap-0.5 bg-muted/30 rounded-md px-2 py-1.5 border border-transparent group-hover:border-primary/5 transition-colors">
                        <span className="text-[9px] uppercase tracking-wider opacity-50">
                          <Trans>Web Push</Trans>
                        </span>
                        <span className="text-[11px] font-medium text-foreground/80">
                          {webPushEnabled ? t`Enabled` : t`Disabled`}
                        </span>
                      </div>
                    </div>
                  </div>

                  <div className="relative z-10 mt-6 pt-4 border-t border-border/10 flex items-center justify-between text-[11px] font-medium text-primary/80 group-hover:text-primary transition-colors">
                    <span>
                      <Trans>Configure Settings</Trans>
                    </span>
                    <Settings2 className="h-3.5 w-3.5 opacity-0 group-hover:opacity-100 transition-all translate-x-2 group-hover:translate-x-0 duration-300" />
                  </div>
                </div>
              </motion.div>

              {/* Other Channels */}
              {(channels || []).map((channel) => (
                <motion.div key={channel.id} variants={item}>
                  <ChannelCard
                    channel={channel}
                    onEdit={handleEdit}
                    onDelete={handleDelete}
                    onTest={handleTest}
                    onManageSubscriptions={handleManageSubscriptions}
                  />
                </motion.div>
              ))}
            </motion.div>
          )}
        </AnimatePresence>
      </div>

      <BrowserChannelDialog
        open={isBrowserDialogOpen}
        onOpenChange={setIsBrowserDialogOpen}
        webPushSupported={webPushSupported}
        webPushEnabled={webPushEnabled}
        webPushPermission={webPushPermission}
        webPushStatusText={webPushStatusText}
        webPushMinPriority={webPushMinPriority}
        onWebPushToggle={toggleWebPush}
        onWebPushPriorityChange={handleWebPushPriorityChange}
        browserNotificationsEnabled={browserNotificationsEnabled}
        onBrowserNotificationsToggle={handleBrowserNotificationsToggle}
      />

      <ChannelForm
        open={isFormOpen}
        onOpenChange={(open) => {
          setIsFormOpen(open);
          if (!open) setEditingChannel(null);
        }}
        channel={editingChannel}
      />

      <SubscriptionManager
        open={isSubsOpen}
        onOpenChange={(open) => {
          setIsSubsOpen(open);
          if (!open) setSubsChannel(null);
        }}
        channel={subsChannel}
      />
    </motion.div>
  );
}
