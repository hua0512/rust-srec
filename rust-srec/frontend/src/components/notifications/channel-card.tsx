import { NotificationChannel } from '@/api/schemas';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from '@/components/ui/alert-dialog';
import {
  MoreHorizontal,
  Edit,
  Trash2,
  BellRing,
  Activity,
  Webhook,
  Mail,
  MessageSquare,
  Send,
} from 'lucide-react';
import { Trans } from '@lingui/react/macro';

interface ChannelCardProps {
  channel: NotificationChannel;
  onEdit: (channel: NotificationChannel) => void;
  onDelete: (id: string) => void;
  onTest: (id: string) => void;
  onManageSubscriptions: (channel: NotificationChannel) => void;
}

const CHANNEL_ICONS = {
  Discord: MessageSquare,
  Email: Mail,
  Telegram: Send,
  Webhook: Webhook,
};

const CHANNEL_COLORS = {
  Discord: 'bg-indigo-500/10 text-indigo-500 border-indigo-500/20',
  Email: 'bg-blue-500/10 text-blue-500 border-blue-500/20',
  Telegram: 'bg-sky-500/10 text-sky-500 border-sky-500/20',
  Webhook: 'bg-orange-500/10 text-orange-500 border-orange-500/20',
};

export function ChannelCard({
  channel,
  onEdit,
  onDelete,
  onTest,
  onManageSubscriptions,
}: ChannelCardProps) {
  const Icon = CHANNEL_ICONS[channel.channel_type] || Webhook;
  const colorClass =
    CHANNEL_COLORS[channel.channel_type] ||
    'bg-primary/10 text-primary border-primary/20';

  let configObj: any = {};
  try {
    if (typeof channel.settings === 'string') {
      configObj = JSON.parse(channel.settings);
    } else {
      configObj = channel.settings;
    }
  } catch {}

  const renderConfigDetails = () => {
    switch (channel.channel_type) {
      case 'Discord':
        return (
          <div className="flex flex-col gap-0.5 bg-muted/30 rounded-md px-2 py-1.5 border border-transparent group-hover:border-primary/5 transition-colors">
            <span className="text-[9px] uppercase tracking-wider opacity-50">
              <Trans>Webhook URL</Trans>
            </span>
            <span className="text-[11px] font-medium truncate text-foreground/80">
              {configObj.webhook_url || '...'}
            </span>
          </div>
        );
      case 'Email':
        return (
          <>
            <div className="flex flex-col gap-0.5 bg-muted/30 rounded-md px-2 py-1.5 border border-transparent group-hover:border-primary/5 transition-colors">
              <span className="text-[9px] uppercase tracking-wider opacity-50">
                <Trans>To</Trans>
              </span>
              <span className="text-[11px] font-medium truncate text-foreground/80">
                {(configObj.to_addresses || []).join(', ')}
              </span>
            </div>
          </>
        );
      case 'Telegram':
        return (
          <>
            <div className="flex flex-col gap-0.5 bg-muted/30 rounded-md px-2 py-1.5 border border-transparent group-hover:border-primary/5 transition-colors">
              <span className="text-[9px] uppercase tracking-wider opacity-50">
                <Trans>Bot Token</Trans>
              </span>
              <span className="text-[11px] font-medium truncate text-foreground/80">
                {configObj.bot_token ? `${configObj.bot_token.slice(0, 6)}${'*'.repeat(20)}` : '...'}
              </span>
            </div>
            <div className="flex flex-col gap-0.5 bg-muted/30 rounded-md px-2 py-1.5 border border-transparent group-hover:border-primary/5 transition-colors">
              <span className="text-[9px] uppercase tracking-wider opacity-50">
                <Trans>Chat ID</Trans>
              </span>
              <span className="text-[11px] font-medium truncate text-foreground/80">
                {configObj.chat_id || '...'}
              </span>
            </div>
          </>
        );
      case 'Webhook':
        return (
          <div className="flex flex-col gap-0.5 bg-muted/30 rounded-md px-2 py-1.5 border border-transparent group-hover:border-primary/5 transition-colors">
            <span className="text-[9px] uppercase tracking-wider opacity-50">
              <Trans>URL</Trans>
            </span>
            <span className="text-[11px] font-medium truncate text-foreground/80">
              {configObj.url || '...'}
            </span>
          </div>
        );
      default:
        return null;
    }
  };

  return (
    <Card className="relative h-full flex flex-col transition-all duration-500 hover:-translate-y-1 hover:shadow-2xl hover:shadow-primary/10 group overflow-hidden bg-gradient-to-br from-background/80 to-background/40 backdrop-blur-xl border-border/40 hover:border-primary/20">
      <div className="absolute inset-x-0 top-0 h-0.5 bg-gradient-to-r from-transparent via-primary/40 to-transparent opacity-0 group-hover:opacity-100 transition-opacity duration-700" />

      {/* Hover Glow Effect */}
      <div className="absolute -inset-0.5 bg-gradient-to-br from-primary/5 to-transparent opacity-0 group-hover:opacity-100 blur-2xl transition-opacity duration-500 pointer-events-none" />

      <CardHeader className="relative flex flex-row items-center gap-4 pb-2 space-y-0 z-10">
        <div
          className={`p-3 rounded-2xl ${colorClass.replace('bg-', 'bg-opacity-10 ')} ring-1 ring-inset ring-black/5 dark:ring-white/5 transition-transform duration-500 group-hover:scale-110 group-hover:rotate-3`}
        >
          <Icon className="h-5 w-5" />
        </div>
        <div className="flex-1 min-w-0 space-y-1">
          <CardTitle className="text-base font-medium truncate tracking-tight text-foreground/90 group-hover:text-primary transition-colors duration-300">
            {channel.name}
          </CardTitle>
          <div className="flex items-center gap-2">
            <span className="text-[10px] uppercase tracking-wider font-semibold text-muted-foreground/60">
              {channel.channel_type}
            </span>
          </div>
        </div>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button
              variant="ghost"
              size="icon"
              className="h-8 w-8 -mr-2 text-muted-foreground/40 hover:text-foreground transition-colors"
            >
              <MoreHorizontal className="h-4 w-4" />
              <span className="sr-only">
                <Trans>Open menu</Trans>
              </span>
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="w-48">
            <DropdownMenuItem onClick={() => onEdit(channel)}>
              <Edit className="mr-2 h-4 w-4" /> <Trans>Edit</Trans>
            </DropdownMenuItem>
            <DropdownMenuItem onClick={() => onManageSubscriptions(channel)}>
              <Activity className="mr-2 h-4 w-4" /> <Trans>Subscriptions</Trans>
            </DropdownMenuItem>
            <DropdownMenuItem onClick={() => onTest(channel.id)}>
              <BellRing className="mr-2 h-4 w-4" />
              <Trans>Test Notification</Trans>
            </DropdownMenuItem>
            <AlertDialog>
              <AlertDialogTrigger asChild>
                <DropdownMenuItem
                  className="text-destructive focus:text-destructive"
                  onSelect={(e) => e.preventDefault()}
                >
                  <Trash2 className="mr-2 h-4 w-4" /> <Trans>Delete</Trans>
                </DropdownMenuItem>
              </AlertDialogTrigger>
              <AlertDialogContent>
                <AlertDialogHeader>
                  <AlertDialogTitle>
                    <Trans>Delete Channel?</Trans>
                  </AlertDialogTitle>
                  <AlertDialogDescription>
                    <Trans>
                      This will permanently delete the notification channel "
                      {channel.name}".
                    </Trans>
                  </AlertDialogDescription>
                </AlertDialogHeader>
                <AlertDialogFooter>
                  <AlertDialogCancel>
                    <Trans>Cancel</Trans>
                  </AlertDialogCancel>
                  <AlertDialogAction
                    onClick={() => onDelete(channel.id)}
                    className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
                  >
                    <Trans>Delete</Trans>
                  </AlertDialogAction>
                </AlertDialogFooter>
              </AlertDialogContent>
            </AlertDialog>
          </DropdownMenuContent>
        </DropdownMenu>
      </CardHeader>
      <CardContent className="relative pb-4 flex-1 z-10">
        <div className="text-sm text-muted-foreground grid gap-2">
          {renderConfigDetails()}
        </div>
      </CardContent>
    </Card>
  );
}
