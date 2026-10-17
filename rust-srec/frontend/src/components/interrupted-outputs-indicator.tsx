import { useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { FileWarning } from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';

import { interruptedOutputsQueryOptions } from '@/api/interrupted-outputs';
import {
  DeleteAllInterruptedOutputs,
  InterruptedOutputList,
  interruptedOutputsLabel,
} from '@/components/pipeline/interrupted-output-list';
import { NotificationBadge } from '@/components/ui/notification-badge';
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from '@/components/ui/popover';
import { usePresence } from '@/hooks/use-presence';
import {
  HEADER_BADGE_DOT,
  HEADER_POPOVER,
  HEADER_TRIGGER_EXIT_MS,
  headerTriggerClass,
} from '@/components/header-indicator';
import { cn } from '@/lib/utils';

/**
 * Header entry for partial temporary files that jobs interrupted by a crash,
 * power loss or forced exit left next to their destinations. Renders nothing
 * while there are none.
 */
export function InterruptedOutputsIndicator() {
  const { i18n } = useLingui();
  const [open, setOpen] = useState(false);
  const { data } = useQuery({
    ...interruptedOutputsQueryOptions(),
    enabled: typeof window !== 'undefined',
    retry: false,
  });
  const outputs = data?.outputs ?? [];
  const totalBytes = data?.total_size_bytes ?? 0;
  const hasContent = outputs.length > 0;
  const { mounted, exiting } = usePresence(hasContent, HEADER_TRIGGER_EXIT_MS);

  if (!mounted) return null;

  const label = interruptedOutputsLabel(outputs.length, totalBytes, i18n);

  return (
    <Popover open={open && hasContent} onOpenChange={setOpen}>
      <PopoverTrigger
        aria-label={label}
        title={label}
        className={cn(
          headerTriggerClass(exiting),
          'text-amber-600 hover:text-amber-600 dark:text-amber-400 dark:hover:text-amber-400',
        )}
      >
        <FileWarning className="size-4" />
        <NotificationBadge
          open={hasContent}
          entering={hasContent}
          className="-top-0.5 -right-0.5"
          dotClassName={cn(HEADER_BADGE_DOT, 'bg-amber-500')}
        >
          {hasContent ? outputs.length : undefined}
        </NotificationBadge>
      </PopoverTrigger>
      <PopoverContent align="end" sideOffset={8} className={HEADER_POPOVER}>
        <div className="space-y-1 px-4 py-3">
          <p className="text-sm font-semibold">{label}</p>
          <p className="text-xs text-muted-foreground">
            <Trans>
              A crash or forced exit stopped these jobs before they could clean
              up. The files are unfinished and safe to delete.
            </Trans>
          </p>
          <div className="pt-1.5">
            <DeleteAllInterruptedOutputs
              outputs={outputs}
              totalBytes={totalBytes}
            />
          </div>
        </div>
        <InterruptedOutputList
          outputs={outputs}
          onNavigate={() => setOpen(false)}
          className="max-h-80 overflow-y-auto border-t border-border/60"
        />
      </PopoverContent>
    </Popover>
  );
}
