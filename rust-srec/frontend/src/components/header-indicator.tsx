import { useState, type ReactNode } from 'react';
import type { LucideIcon } from 'lucide-react';

import { buttonVariants } from '@/components/ui/button';
import { NotificationBadge } from '@/components/ui/notification-badge';
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from '@/components/ui/popover';
import { usePresence } from '@/hooks/use-presence';
import { cn } from '@/lib/utils';

/**
 * How long a header indicator's trigger takes to collapse once it has nothing
 * to show. The exit `animation-duration` in `headerTriggerClass` must match.
 */
export const HEADER_TRIGGER_EXIT_MS = 180;

/**
 * The round trigger of a header indicator, with the notification-badge pop
 * applied to the whole trigger: it springs in when the indicator appears and
 * collapses without overshoot when it goes away.
 */
export function headerTriggerClass(exiting: boolean) {
  return cn(
    buttonVariants({ variant: 'ghost', size: 'icon' }),
    'relative h-9 w-9 rounded-full motion-reduce:animate-none',
    exiting
      ? 'pointer-events-none animate-out fill-mode-forwards fade-out-0 zoom-out-60 blur-out-[2px] animation-duration-180 ease-[cubic-bezier(0.4,0,0.2,1)]'
      : 'animate-in fade-in-0 zoom-in-60 blur-in-[2px] animation-duration-500 ease-[cubic-bezier(0.34,1.36,0.64,1)]',
  );
}

/** The count on a header indicator's trigger; the caller adds its colour. */
export const HEADER_BADGE_DOT =
  'h-4 min-w-4 rounded-full px-1 text-[10px] leading-none font-semibold text-white tabular-nums';

/** The popover a header indicator opens. */
export const HEADER_POPOVER =
  'w-80 overflow-hidden rounded-xl border-border/60 bg-popover/80 p-0 shadow-xl backdrop-blur-xl ease-[cubic-bezier(0.22,1,0.36,1)] data-[side=bottom]:slide-in-from-top-0 data-[state=closed]:duration-150 data-[state=closed]:zoom-out-99 data-[state=open]:duration-250 data-[state=open]:zoom-in-97 motion-reduce:animate-none';

const TONES = {
  amber: {
    trigger:
      'text-amber-600 hover:text-amber-600 dark:text-amber-400 dark:hover:text-amber-400',
    badge: 'bg-amber-500',
  },
  orange: {
    trigger:
      'text-orange-600 hover:text-orange-600 dark:text-orange-400 dark:hover:text-orange-400',
    badge: 'bg-orange-500',
  },
} as const;

/**
 * A header entry shown while `count` is above zero: a round trigger with a
 * count badge that opens a popover headed by `label` and `description`.
 * `children` renders the popover's scrolling list and gets `close` for entries
 * that navigate away.
 */
export function HeaderIndicator({
  count,
  label,
  icon: Icon,
  tone,
  description,
  actions,
  children,
}: {
  count: number;
  label: string;
  icon: LucideIcon;
  tone: keyof typeof TONES;
  description: ReactNode;
  /** Controls under the description, such as a bulk action. */
  actions?: ReactNode;
  children: (close: () => void) => ReactNode;
}) {
  const [open, setOpen] = useState(false);
  const hasContent = count > 0;
  const { mounted, exiting } = usePresence(hasContent, HEADER_TRIGGER_EXIT_MS);

  if (!mounted) return null;

  const colors = TONES[tone];

  return (
    <Popover open={open && hasContent} onOpenChange={setOpen}>
      <PopoverTrigger
        aria-label={label}
        title={label}
        className={cn(headerTriggerClass(exiting), colors.trigger)}
      >
        <Icon className="size-4" />
        <NotificationBadge
          open={hasContent}
          entering={hasContent}
          className="-top-0.5 -right-0.5"
          dotClassName={cn(HEADER_BADGE_DOT, colors.badge)}
        >
          {hasContent ? count : undefined}
        </NotificationBadge>
      </PopoverTrigger>
      <PopoverContent align="end" sideOffset={8} className={HEADER_POPOVER}>
        <div className="space-y-1 px-4 py-3">
          <p className="text-sm font-semibold">{label}</p>
          <p className="text-xs text-muted-foreground">{description}</p>
          {actions && <div className="pt-1.5">{actions}</div>}
        </div>
        <div className="max-h-80 overflow-y-auto border-t border-border/60">
          {children(() => setOpen(false))}
        </div>
      </PopoverContent>
    </Popover>
  );
}
