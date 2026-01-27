import { Monitor, Info, TestTubeDiagonal } from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';

import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
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
import { Checkbox } from '@/components/ui/checkbox';
import { cn } from '@/lib/utils';

import type { NotificationEventTypeInfo } from '@/api/schemas/notifications';
import type {
  DesktopNotificationConfig,
  DesktopNotificationMinPriority,
} from '@/desktop/desktop-notifications';

interface DesktopChannelDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  config: DesktopNotificationConfig;
  eventTypes: NotificationEventTypeInfo[];
  onConfigChange: (next: DesktopNotificationConfig) => void;
  onTest: () => void;
}

export function DesktopChannelDialog({
  open,
  onOpenChange,
  config,
  eventTypes,
  onConfigChange,
  onTest,
}: DesktopChannelDialogProps) {
  const { i18n } = useLingui();

  const priorityLabel = (p: DesktopNotificationMinPriority) => {
    switch (p) {
      case 'critical':
        return i18n._(msg`Critical Only`);
      case 'high':
        return i18n._(msg`High+`);
      case 'normal':
        return i18n._(msg`Normal+`);
      case 'low':
        return i18n._(msg`All`);
    }
  };

  const toggleEventType = (eventType: string, checked: boolean) => {
    const current = new Set(config.eventTypes);
    if (checked) {
      current.add(eventType);
    } else {
      current.delete(eventType);
    }
    onConfigChange({ ...config, eventTypes: Array.from(current) });
  };

  const allSelected =
    eventTypes.length > 0 && config.eventTypes.length === eventTypes.length;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-[520px]">
        <DialogHeader>
          <div className="flex items-center gap-3 mb-2">
            <div className="p-2 rounded-xl bg-primary/10 ring-1 ring-primary/20">
              <Monitor className="h-5 w-5 text-primary" />
            </div>
            <DialogTitle className="text-xl font-bold tracking-tight">
              <Trans>Desktop Notifications</Trans>
            </DialogTitle>
          </div>
          <DialogDescription className="text-xs text-muted-foreground/60">
            <Trans>
              Configure native OS notifications for important events while
              running the desktop app.
            </Trans>
          </DialogDescription>
        </DialogHeader>

        <div className="py-6 space-y-6">
          <div className="space-y-4">
            <div className="flex items-center justify-between">
              <div className="flex items-center gap-2.5">
                <div
                  className={cn(
                    'h-2 w-2 rounded-full transition-all duration-500',
                    config.enabled
                      ? 'bg-green-500 shadow-[0_0_8px_rgba(34,197,94,0.5)]'
                      : 'bg-muted-foreground/30',
                  )}
                />
                <span className="text-sm font-semibold tracking-tight">
                  <Trans>Enabled</Trans>
                </span>
                <Badge
                  variant="outline"
                  className="text-[9px] h-4 py-0 border-primary/20 text-primary"
                >
                  <Trans>Native</Trans>
                </Badge>
              </div>
              <Switch
                checked={config.enabled}
                onCheckedChange={(enabled) =>
                  onConfigChange({ ...config, enabled })
                }
              />
            </div>
            <p className="text-[11px] text-muted-foreground leading-relaxed pl-4.5">
              <Trans>
                Desktop notifications are shown by the operating system even
                when the app is minimized.
              </Trans>
            </p>
          </div>

          <div className="h-px bg-border/40" />

          <div className="space-y-3">
            <div className="flex items-center justify-between text-[11px]">
              <span className="text-muted-foreground">
                <Trans>Minimum Priority</Trans>
              </span>
              <Select
                value={config.minPriority}
                onValueChange={(minPriority) =>
                  onConfigChange({
                    ...config,
                    minPriority: minPriority as DesktopNotificationMinPriority,
                  })
                }
              >
                <SelectTrigger className="h-7 w-40 bg-background/40 border-border/20 text-[10px]">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="critical" className="text-[10px]">
                    {priorityLabel('critical')}
                  </SelectItem>
                  <SelectItem value="high" className="text-[10px]">
                    {priorityLabel('high')}
                  </SelectItem>
                  <SelectItem value="normal" className="text-[10px]">
                    {priorityLabel('normal')}
                  </SelectItem>
                  <SelectItem value="low" className="text-[10px]">
                    {priorityLabel('low')}
                  </SelectItem>
                </SelectContent>
              </Select>
            </div>

            <div className="space-y-2">
              <div className="flex items-center justify-between">
                <span className="text-[11px] text-muted-foreground">
                  <Trans>Event Types</Trans>
                </span>
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  className="h-7 text-[10px]"
                  onClick={() => {
                    onConfigChange({
                      ...config,
                      eventTypes: allSelected
                        ? []
                        : eventTypes.map((e) => e.event_type),
                    });
                  }}
                >
                  {allSelected ? i18n._(msg`Clear`) : i18n._(msg`Select All`)}
                </Button>
              </div>

              <div className="max-h-56 overflow-auto rounded-xl border border-border/30 bg-background/30 p-2">
                {eventTypes.map((e) => {
                  const checked = config.eventTypes.includes(e.event_type);
                  return (
                    <label
                      key={e.event_type}
                      className="flex items-start gap-2 px-2 py-1.5 rounded-lg hover:bg-muted/30 transition-colors"
                    >
                      <Checkbox
                        checked={checked}
                        onCheckedChange={(v) =>
                          toggleEventType(e.event_type, v === true)
                        }
                        className="mt-0.5"
                      />
                      <div className="min-w-0">
                        <div className="text-[11px] font-medium truncate">
                          {e.label}
                        </div>
                        <div className="text-[10px] text-muted-foreground/70 truncate">
                          {e.event_type}
                        </div>
                      </div>
                    </label>
                  );
                })}
              </div>
            </div>
          </div>

          <div className="p-3 rounded-xl bg-primary/5 border border-primary/10">
            <div className="flex gap-2.5">
              <Info className="h-3.5 w-3.5 text-primary shrink-0 mt-0.5" />
              <p className="text-[10px] text-primary/80 leading-relaxed font-medium">
                <Trans>
                  These settings only apply to the desktop app. The server will
                  still deliver channel notifications (Discord/Email/Webhook)
                  normally.
                </Trans>
              </p>
            </div>
          </div>
        </div>

        <DialogFooter className="gap-2">
          <Button type="button" variant="outline" size="sm" onClick={onTest}>
            <TestTubeDiagonal className="mr-2 h-4 w-4" />
            <Trans>Test</Trans>
          </Button>
          <Button variant="ghost" size="sm" onClick={() => onOpenChange(false)}>
            <Trans>Close</Trans>
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
