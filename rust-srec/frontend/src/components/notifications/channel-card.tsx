import { NotificationChannel } from '@/api/schemas';
import { CardContent } from '@/components/ui/card';
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
  Edit,
  Trash2,
  Bell,
  BellRing,
  Activity,
  Webhook,
  Mail,
  MessageSquare,
  Send,
} from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { CardField } from '@/components/shared/card-field';
import {
  CardHeaderRow,
  CardIconTile,
  CardMenuButton,
  DecoratedCard,
} from '@/components/shared/decorated-card';

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
  Gotify: Bell,
  Telegram: Send,
  Webhook: Webhook,
};

const CHANNEL_COLORS = {
  Discord: 'bg-indigo-500/10 text-indigo-500 border-indigo-500/20',
  Email: 'bg-blue-500/10 text-blue-500 border-blue-500/20',
  Gotify: 'bg-green-500/10 text-green-500 border-green-500/20',
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
          <CardField
            label={<Trans>Webhook URL</Trans>}
            value={configObj.webhook_url || '...'}
          />
        );
      case 'Email':
        return (
          <CardField
            label={<Trans>To</Trans>}
            value={(configObj.to_addresses || []).join(', ')}
          />
        );
      case 'Telegram':
        return (
          <>
            <CardField
              label={<Trans>Bot Token</Trans>}
              value={
                configObj.bot_token
                  ? `${configObj.bot_token.slice(0, 6)}${'*'.repeat(20)}`
                  : '...'
              }
            />
            <CardField
              label={<Trans>Chat ID</Trans>}
              value={configObj.chat_id || '...'}
            />
          </>
        );
      case 'Gotify':
        return (
          <CardField
            label={<Trans>Server URL</Trans>}
            value={configObj.server_url || '...'}
          />
        );
      case 'Webhook':
        return (
          <CardField
            label={<Trans>URL</Trans>}
            value={configObj.url || '...'}
          />
        );
      default:
        return null;
    }
  };

  return (
    <DecoratedCard>
      <CardHeaderRow
        icon={<CardIconTile icon={Icon} className={colorClass} />}
        title={channel.name}
        subtitle={channel.channel_type}
        actions={
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <CardMenuButton />
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" className="w-48">
              <DropdownMenuItem onClick={() => onEdit(channel)}>
                <Edit className="mr-2 h-4 w-4" /> <Trans>Edit</Trans>
              </DropdownMenuItem>
              <DropdownMenuItem onClick={() => onManageSubscriptions(channel)}>
                <Activity className="mr-2 h-4 w-4" />{' '}
                <Trans>Subscriptions</Trans>
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
        }
      />
      <CardContent className="relative pb-4 flex-1 z-10">
        <div className="text-sm text-muted-foreground grid grid-cols-1 gap-2">
          {renderConfigDetails()}
        </div>
      </CardContent>
    </DecoratedCard>
  );
}
