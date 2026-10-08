import { buttonVariants } from '@/components/ui/button';
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
