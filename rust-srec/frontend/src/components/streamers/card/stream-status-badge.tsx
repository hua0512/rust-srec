import type { ReactNode } from 'react';
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from '@/components/ui/tooltip';
import { cn } from '../../../lib/utils';
import type { useStreamerStatus } from './use-streamer-status';

/** A pill beside a streamer's name, with its details in a tooltip below. */
export function StatusPill({
  className,
  tooltip,
  children,
}: {
  /** Spacing and colours. */
  className: string;
  tooltip?: ReactNode;
  children: ReactNode;
}) {
  return (
    <TooltipProvider delayDuration={200}>
      <Tooltip>
        <TooltipTrigger asChild>
          <div
            className={cn(
              'flex items-center h-6 px-2 rounded-full border text-[10px] font-medium transition-all cursor-help select-none',
              className,
            )}
          >
            {children}
          </div>
        </TooltipTrigger>
        {tooltip && (
          <TooltipContent
            className="p-0 border-border/50 shadow-xl bg-background/95 backdrop-blur-md overflow-hidden"
            arrowClassName="bg-background fill-background"
            side="bottom"
            align="start"
          >
            {tooltip}
          </TooltipContent>
        )}
      </Tooltip>
    </TooltipProvider>
  );
}

interface StatusBadgeProps {
  status: ReturnType<typeof useStreamerStatus>;
}

export const StatusBadge = ({ status }: StatusBadgeProps) => (
  <StatusPill
    className={cn('gap-1.5 pr-2.5', status.color)}
    tooltip={status.tooltip}
  >
    {status.variant === 'live' ? (
      <span className="relative flex h-2 w-2 mr-1.5">
        <span
          className={cn(
            'animate-ping absolute inline-flex h-full w-full rounded-full opacity-75',
            status.pingColor,
          )}
        ></span>
        <span
          className={cn(
            'relative inline-flex rounded-full h-2 w-2',
            status.iconColor,
          )}
        ></span>
      </span>
    ) : (
      <span
        className={cn(
          'h-1.5 w-1.5 rounded-full min-w-[6px]',
          status.iconColor,
          status.pulsing &&
            'animate-pulse shadow-[0_0_8px_rgba(239,68,68,0.6)]',
        )}
      />
    )}
    {status.label}
  </StatusPill>
);
