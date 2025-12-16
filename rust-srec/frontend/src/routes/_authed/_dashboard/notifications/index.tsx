import { useState } from 'react';
import { createFileRoute } from '@tanstack/react-router';
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import {
  listChannels,
  deleteChannel,
  testChannel,
} from '@/server/functions/notifications';
import { Button } from '@/components/ui/button';
import { Skeleton } from '@/components/ui/skeleton';
import { Plus, Bell, Settings2 } from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { t } from '@lingui/core/macro';
import { toast } from 'sonner';
import { ChannelCard } from '@/components/notifications/channel-card';
import { ChannelForm } from '@/components/notifications/channel-form';
import { SubscriptionManager } from '@/components/notifications/subscription-manager';
import { NotificationChannel } from '@/api/schemas';
import { motion, AnimatePresence } from 'motion/react';
import { Badge } from '@/components/ui/badge';

export const Route = createFileRoute('/_authed/_dashboard/notifications/')({
  component: NotificationsPage,
});

function NotificationsPage() {
  const queryClient = useQueryClient();
  const [isFormOpen, setIsFormOpen] = useState(false);
  const [editingChannel, setEditingChannel] =
    useState<NotificationChannel | null>(null);
  const [isSubsOpen, setIsSubsOpen] = useState(false);
  const [subsChannel, setSubsChannel] =
    useState<NotificationChannel | null>(null);

  const { data: channels, isLoading } = useQuery({
    queryKey: ['notification-channels'],
    queryFn: () => listChannels(),
  });

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
                {channels?.length || 0} <Trans>channels</Trans>
              </Badge>
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
              {[1, 2, 3].map((i) => (
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
          ) : channels && channels.length > 0 ? (
            <motion.div
              key="list"
              variants={container}
              initial="hidden"
              animate="show"
              className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 gap-6"
            >
              {channels.map((channel) => (
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
          ) : (
            <motion.div
              key="empty"
              initial={{ opacity: 0, scale: 0.95 }}
              animate={{ opacity: 1, scale: 1 }}
              className="flex flex-col items-center justify-center py-32 text-center space-y-6 border-2 border-dashed border-muted-foreground/20 rounded-2xl bg-muted/5 backdrop-blur-sm shadow-sm"
            >
              <div className="p-6 bg-primary/5 rounded-full ring-1 ring-primary/10">
                <Settings2 className="h-16 w-16 text-primary/60" />
              </div>
              <div className="space-y-2 max-w-md">
                <h3 className="font-semibold text-2xl tracking-tight">
                  <Trans>No notification channels</Trans>
                </h3>
                <p className="text-muted-foreground">
                  <Trans>
                    Create a channel to receive alerts about downloads, errors,
                    and system events.
                  </Trans>
                </p>
              </div>
              <Button onClick={handleCreate} size="lg" className="mt-4">
                <Plus className="mr-2 h-5 w-5" />
                <Trans>Create Channel</Trans>
              </Button>
            </motion.div>
          )}
        </AnimatePresence>
      </div>

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
