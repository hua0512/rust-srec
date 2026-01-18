import { Monitor, Shield, Info } from 'lucide-react';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
  DialogFooter,
} from '@/components/ui/dialog';
import { Button } from '@/components/ui/button';
import { Badge } from '@/components/ui/badge';
import { Switch } from '@/components/ui/switch';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { cn } from '@/lib/utils';

interface BrowserChannelDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  webPushSupported: boolean;
  webPushEnabled: boolean;
  webPushPermission: NotificationPermission;
  webPushStatusText: string;
  webPushMinPriority: string;
  onWebPushToggle: (checked: boolean) => void;
  onWebPushPriorityChange: (value: string) => void;
  browserNotificationsEnabled: boolean;
  onBrowserNotificationsToggle: (enabled: boolean) => void;
}

export function BrowserChannelDialog({
  open,
  onOpenChange,
  webPushSupported,
  webPushEnabled,
  webPushPermission,
  webPushStatusText,
  webPushMinPriority,
  onWebPushToggle,
  onWebPushPriorityChange,
  browserNotificationsEnabled,
  onBrowserNotificationsToggle,
}: BrowserChannelDialogProps) {
  const { i18n } = useLingui();
  const isWebPushReady = webPushSupported && webPushPermission === 'granted';

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-[425px]">
        <DialogHeader>
          <div className="flex items-center gap-3 mb-2">
            <div className="p-2 rounded-xl bg-primary/10 ring-1 ring-primary/20">
              <Monitor className="h-5 w-5 text-primary" />
            </div>
            <DialogTitle className="text-xl font-bold tracking-tight">
              <Trans>Browser Notifications</Trans>
            </DialogTitle>
          </div>
          <DialogDescription className="text-xs text-muted-foreground/60">
            <Trans>
              Configure how you receive alerts in this browser and via system
              push notifications.
            </Trans>
          </DialogDescription>
        </DialogHeader>

        <div className="py-6 space-y-6">
          {/* Polling Notifications */}
          <div className="space-y-4">
            <div className="flex items-center justify-between">
              <div className="flex items-center gap-2.5">
                <div className="h-2 w-2 rounded-full bg-blue-500 animate-pulse shadow-[0_0_8px_rgba(59,130,246,0.5)]" />
                <span className="text-sm font-semibold tracking-tight">
                  <Trans>Live Polling</Trans>
                </span>
                <Badge
                  variant="outline"
                  className="text-[9px] h-4 py-0 border-blue-500/20 text-blue-500"
                >
                  <Trans>Real-time</Trans>
                </Badge>
              </div>
              <Switch
                checked={browserNotificationsEnabled}
                onCheckedChange={onBrowserNotificationsToggle}
              />
            </div>
            <p className="text-[11px] text-muted-foreground leading-relaxed pl-4.5">
              <Trans>
                Shows notifications while the application tab is open by polling
                recent events.
              </Trans>
            </p>
          </div>

          <div className="h-px bg-border/40" />

          {/* Web Push */}
          <div className="space-y-4">
            <div className="flex items-center justify-between">
              <div className="flex items-center gap-2.5">
                <div
                  className={cn(
                    'h-2 w-2 rounded-full transition-all duration-500',
                    isWebPushReady
                      ? 'bg-green-500 shadow-[0_0_8px_rgba(34,197,94,0.5)]'
                      : 'bg-muted-foreground/30',
                  )}
                />
                <span className="text-sm font-semibold tracking-tight">
                  <Trans>Web Push</Trans>
                </span>
                <Badge
                  variant="outline"
                  className="text-[9px] h-4 py-0 border-green-500/20 text-green-500"
                >
                  <Trans>Background</Trans>
                </Badge>
              </div>
              <Switch
                checked={webPushEnabled}
                onCheckedChange={onWebPushToggle}
                disabled={!webPushSupported}
              />
            </div>

            <div className="space-y-3 pl-4.5">
              <div className="flex items-center justify-between text-[11px]">
                <span className="text-muted-foreground">
                  <Trans>Priority Filter</Trans>
                </span>
                <Select
                  value={webPushMinPriority}
                  onValueChange={onWebPushPriorityChange}
                  disabled={!webPushSupported}
                >
                  <SelectTrigger className="h-7 w-32 bg-background/40 border-border/20 text-[10px]">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="critical" className="text-[10px]">
                      {i18n._(msg`Critical Only`)}
                    </SelectItem>
                    <SelectItem value="high" className="text-[10px]">
                      {i18n._(msg`High+`)}
                    </SelectItem>
                    <SelectItem value="normal" className="text-[10px]">
                      {i18n._(msg`Normal+`)}
                    </SelectItem>
                    <SelectItem value="low" className="text-[10px]">
                      {i18n._(msg`All`)}
                    </SelectItem>
                  </SelectContent>
                </Select>
              </div>

              <div className="flex items-center justify-between text-[11px] p-2 rounded-lg bg-background/40 border border-border/20">
                <div className="flex items-center gap-2 text-muted-foreground">
                  <Shield className="h-3 w-3" />
                  <span>
                    <Trans>Browser Status</Trans>
                  </span>
                </div>
                <span
                  className={cn(
                    'font-medium',
                    webPushPermission === 'granted'
                      ? 'text-green-500'
                      : 'text-amber-500',
                  )}
                >
                  {webPushStatusText}
                </span>
              </div>
            </div>
          </div>

          {/* Info Box */}
          <div className="p-3 rounded-xl bg-primary/5 border border-primary/10">
            <div className="flex gap-2.5">
              <Info className="h-3.5 w-3.5 text-primary shrink-0 mt-0.5" />
              <p className="text-[10px] text-primary/80 leading-relaxed font-medium">
                <Trans>
                  Web Push allows receiving notifications even when you close
                  the browser. It requires browser permission and valid server
                  keys.
                </Trans>
              </p>
            </div>
          </div>
        </div>

        <DialogFooter>
          <Button variant="ghost" size="sm" onClick={() => onOpenChange(false)}>
            <Trans>Close</Trans>
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
