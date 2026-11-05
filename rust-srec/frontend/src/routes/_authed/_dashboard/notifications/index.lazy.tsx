import { useMemo, useState, useSyncExternalStore } from 'react';
import { createLazyFileRoute } from '@tanstack/react-router';
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import {
  listChannels,
  deleteChannel,
  testChannel,
  listEventTypes,
} from '@/server/functions/notifications';
import { Button } from '@/components/ui/button';
import { Skeleton } from '@/components/ui/skeleton';
import { CardSkeleton } from '@/components/shared/card-skeleton';
import { DashboardHeader } from '@/components/shared/dashboard-header';
import { Plus, Bell, Settings2, ListOrdered, Monitor } from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { msg } from '@lingui/core/macro';
import { toast } from 'sonner';
import { ChannelCard } from '@/components/notifications/channel-card';
import { ChannelForm } from '@/components/notifications/channel-form';
import { SubscriptionManager } from '@/components/notifications/subscription-manager';
import { BrowserChannelDialog } from '@/components/notifications/browser-channel-dialog';
import { DesktopChannelDialog } from '@/components/notifications/desktop-channel-dialog';
import { NotificationChannel } from '@/api/schemas';
import { motion, AnimatePresence } from 'motion/react';
import { Badge } from '@/components/ui/badge';
import { Link } from '@tanstack/react-router';
import { cn } from '@/lib/utils';
import { containerVariants, itemVariants } from '@/lib/animation';
import { minPriorityLabel } from '@/lib/priority';
import { useNotificationDot } from '@/hooks/use-notification-dot';
import { isTauriRuntime } from '@/utils/tauri';
import { useDesktopNotificationConfig } from '@/hooks/use-desktop-notification-config';
import { useWebPushSubscription } from '@/hooks/use-web-push-subscription';
import {
  getBrowserNotificationsEnabled,
  onNotificationStateChanged,
  setBrowserNotificationsEnabled,
} from '@/lib/notification-state';
import { testDesktopNotifications } from '@/desktop/desktop-notifications';
import { CARD_GRID } from '@/lib/grid';
import { CardField } from '@/components/shared/card-field';
import {
  CardDecoration,
  CardIconTile,
} from '@/components/shared/decorated-card';

const STATUS_VALUE_CLASS =
  'text-base text-foreground tracking-tight group-hover:text-primary transition-colors duration-300';

export const Route = createLazyFileRoute('/_authed/_dashboard/notifications/')({
  component: NotificationsPage,
});

function NotificationsPage() {
  const isDesktop = isTauriRuntime();
  const queryClient = useQueryClient();
  const { i18n } = useLingui();
  const [isFormOpen, setIsFormOpen] = useState(false);
  const [editingChannel, setEditingChannel] =
    useState<NotificationChannel | null>(null);
  const [isSubsOpen, setIsSubsOpen] = useState(false);
  const [subsChannel, setSubsChannel] = useState<NotificationChannel | null>(
    null,
  );

  const webPush = useWebPushSubscription({ enabled: !isDesktop });

  const [isBrowserDialogOpen, setIsBrowserDialogOpen] = useState(false);
  const browserNotificationsEnabled = useSyncExternalStore(
    onNotificationStateChanged,
    getBrowserNotificationsEnabled,
    () => false,
  );

  const [isDesktopDialogOpen, setIsDesktopDialogOpen] = useState(false);
  const openInternalChannelSettings = () =>
    isDesktop ? setIsDesktopDialogOpen(true) : setIsBrowserDialogOpen(true);

  const [desktopNotificationsConfig, updateDesktopNotificationsConfig] =
    useDesktopNotificationConfig(isDesktop);

  const { hasCriticalDot } = useNotificationDot();

  const handleBrowserNotificationsToggle = (enabled: boolean) => {
    setBrowserNotificationsEnabled(enabled);
    if (enabled) {
      toast.success(i18n._(msg`Live polling notifications enabled`));
    }
  };

  const { data: channels, isLoading } = useQuery({
    queryKey: ['notification-channels'],
    queryFn: () => listChannels(),
  });

  const { data: desktopEventTypes = [] } = useQuery({
    queryKey: ['notification-event-types'],
    queryFn: () => listEventTypes(),
    enabled: isDesktop,
  });

  const deleteMutation = useMutation({
    mutationFn: (id: string) => deleteChannel({ data: id }),
    onSuccess: () => {
      toast.success(i18n._(msg`Channel deleted`));
      void queryClient.invalidateQueries({
        queryKey: ['notification-channels'],
      });
    },
    onError: (err) => {
      toast.error(err.message || i18n._(msg`Failed to delete channel`));
    },
  });

  const testMutation = useMutation({
    mutationFn: (id: string) => testChannel({ data: id }),
    onSuccess: () => {
      toast.success(i18n._(msg`Test notification sent`));
    },
    onError: (err) => {
      toast.error(err.message || i18n._(msg`Failed to send test notification`));
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

  const desktopMinPriorityLabel = i18n._(
    minPriorityLabel(desktopNotificationsConfig?.minPriority ?? 5),
  );

  const desktopConfiguredEventsText = useMemo(() => {
    const selected = desktopNotificationsConfig?.eventTypes?.length ?? 0;
    const total = desktopEventTypes.length;
    if (total > 0) return `${selected} / ${total}`;
    return String(selected);
  }, [desktopEventTypes.length, desktopNotificationsConfig?.eventTypes]);

  const internalActive = isDesktop
    ? !!desktopNotificationsConfig?.enabled
    : browserNotificationsEnabled || webPush.subscribed;

  return (
    <motion.div
      className="min-h-screen space-y-6"
      variants={containerVariants}
      initial="hidden"
      animate="visible"
    >
      <DashboardHeader
        variants={itemVariants}
        icon={Bell}
        title={<Trans>Notifications</Trans>}
        subtitle={<Trans>Manage where system notifications are sent</Trans>}
        actions={
          <>
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
          </>
        }
      />

      <div className="p-4 md:px-8 pb-20 w-full">
        <AnimatePresence mode="wait">
          {isLoading ? (
            <motion.div
              key="loading"
              initial={false}
              animate={{ opacity: 1 }}
              exit={{ opacity: 0, transition: { duration: 0.1 } }}
              className={cn(CARD_GRID, 'gap-6')}
            >
              {[1, 2, 3, 4].map((i) => (
                <CardSkeleton key={i}>
                  <div className="flex justify-between items-start">
                    <Skeleton className="h-10 w-10 rounded-full" />
                    <Skeleton className="h-6 w-8" />
                  </div>
                  <div className="space-y-2 pt-2">
                    <Skeleton className="h-6 w-3/4" />
                    <Skeleton className="h-4 w-1/2" />
                  </div>
                </CardSkeleton>
              ))}
            </motion.div>
          ) : (
            <motion.div
              key="list"
              variants={containerVariants}
              initial="hidden"
              animate="visible"
              exit="exit"
              className={cn(CARD_GRID, 'gap-6')}
            >
              {/* Browser Card First */}
              <motion.div variants={itemVariants}>
                {/* The whole card opens the settings for a pointer; the footer
                    button is the keyboard and screen-reader way in, and its
                    click bubbles to the same idempotent handler. */}
                <div
                  onClick={openInternalChannelSettings}
                  className="cursor-pointer has-[:focus-visible]:ring-2 has-[:focus-visible]:ring-ring relative overflow-hidden group border border-border/40 hover:border-primary/20 transition-all duration-500 hover:shadow-2xl hover:shadow-primary/10 bg-gradient-to-br from-background/80 to-background/40 backdrop-blur-xl rounded-3xl p-6 h-full flex flex-col justify-between min-h-[220px]"
                >
                  <CardDecoration />

                  <div className="relative z-10">
                    <div className="flex justify-between items-start mb-4">
                      <CardIconTile
                        icon={Monitor}
                        className="bg-primary/10 text-primary"
                      />
                      <Badge
                        variant={internalActive ? 'default' : 'secondary'}
                        className={cn(
                          'text-[10px] font-bold uppercase tracking-wider px-2 py-0 h-5 border-none shadow-none',
                          internalActive
                            ? 'bg-primary/20 text-primary hover:bg-primary/30'
                            : 'bg-muted/30 text-muted-foreground',
                        )}
                      >
                        {internalActive
                          ? i18n._(msg`Active`)
                          : i18n._(msg`Inactive`)}
                      </Badge>
                    </div>
                    <div className="space-y-1">
                      <h3 className="text-base font-medium tracking-tight group-hover:text-primary transition-colors duration-300">
                        {isDesktop ? (
                          <Trans>Desktop Notifications</Trans>
                        ) : (
                          <Trans>Browser Notifications</Trans>
                        )}
                      </h3>
                      <div className="text-[10px] uppercase tracking-wider font-semibold text-muted-foreground/60">
                        <Trans>Internal</Trans>
                      </div>
                    </div>

                    <div className="mt-4 space-y-2">
                      {isDesktop ? (
                        <>
                          <CardField
                            label={<Trans>Status</Trans>}
                            value={
                              desktopNotificationsConfig?.enabled
                                ? i18n._(msg`Enabled`)
                                : i18n._(msg`Disabled`)
                            }
                            valueClassName={STATUS_VALUE_CLASS}
                          />

                          <div className="grid grid-cols-2 gap-2">
                            <CardField
                              label={<Trans>Configured Events</Trans>}
                              value={desktopConfiguredEventsText}
                            />
                            <CardField
                              label={<Trans>Min Priority</Trans>}
                              value={desktopMinPriorityLabel}
                            />
                          </div>
                        </>
                      ) : (
                        <>
                          <CardField
                            label={<Trans>Live Polling</Trans>}
                            value={
                              browserNotificationsEnabled
                                ? i18n._(msg`Enabled`)
                                : i18n._(msg`Disabled`)
                            }
                            valueClassName={STATUS_VALUE_CLASS}
                          />
                          <CardField
                            label={<Trans>Web Push</Trans>}
                            value={
                              webPush.subscribed
                                ? i18n._(msg`Enabled`)
                                : i18n._(msg`Disabled`)
                            }
                          />
                        </>
                      )}
                    </div>
                  </div>

                  <button
                    type="button"
                    onClick={openInternalChannelSettings}
                    className="relative z-10 mt-6 pt-4 w-full border-t border-border/10 flex items-center justify-between text-[11px] font-medium text-primary/80 group-hover:text-primary focus-visible:text-primary focus-visible:outline-none transition-colors"
                  >
                    <span>
                      <Trans>Configure Settings</Trans>
                    </span>
                    <Settings2 className="h-3.5 w-3.5 opacity-0 group-hover:opacity-100 group-focus-within:opacity-100 transition-all translate-x-2 group-hover:translate-x-0 group-focus-within:translate-x-0 duration-300" />
                  </button>
                </div>
              </motion.div>

              {/* Other Channels */}
              {(channels || []).map((channel) => (
                <motion.div key={channel.id} variants={itemVariants}>
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
        open={!isDesktop && isBrowserDialogOpen}
        onOpenChange={setIsBrowserDialogOpen}
        webPushSupported={webPush.supported}
        webPushEnabled={webPush.subscribed}
        webPushPermission={webPush.permission}
        webPushStatusText={webPush.statusText}
        webPushMinPriority={webPush.minPriority}
        onWebPushToggle={webPush.toggle}
        onWebPushPriorityChange={webPush.changePriority}
        browserNotificationsEnabled={browserNotificationsEnabled}
        onBrowserNotificationsToggle={handleBrowserNotificationsToggle}
      />

      {isDesktop && desktopNotificationsConfig && (
        <DesktopChannelDialog
          open={isDesktopDialogOpen}
          onOpenChange={setIsDesktopDialogOpen}
          config={desktopNotificationsConfig}
          eventTypes={desktopEventTypes}
          onTest={() => {
            void testDesktopNotifications().catch((error: unknown) => {
              toast.error(i18n._(msg`Failed to send test notification`), {
                description:
                  error instanceof Error ? error.message : String(error),
              });
            });
          }}
          onConfigChange={updateDesktopNotificationsConfig}
        />
      )}

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
