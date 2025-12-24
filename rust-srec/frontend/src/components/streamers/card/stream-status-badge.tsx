import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from '@/components/ui/tooltip';
import { cn } from '../../../lib/utils';
import { ReactNode } from 'react';

export interface StatusBadgeProps {
  status: {
    label: ReactNode;
    color: string;
    iconColor: string;
    pulsing: boolean;
    pingColor?: string;
    variant?: string;
    tooltip: ReactNode | null;
  };
}

export const StatusBadge = ({ status }: StatusBadgeProps) => (
  <TooltipProvider delayDuration={200}>
    <Tooltip>
      <TooltipTrigger asChild>
        <div
          className={cn(
            'flex items-center gap-1.5 h-6 px-2 pr-2.5 rounded-full border text-[10px] font-medium transition-all cursor-help select-none',
            status.color,
          )}
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
        </div>
      </TooltipTrigger>
      {status.tooltip && (
        <TooltipContent
          className="p-0 border-border/50 shadow-xl bg-background/95 backdrop-blur-md overflow-hidden"
          side="bottom"
          align="start"
        >
          {status.tooltip}
        </TooltipContent>
      )}
    </Tooltip>
  </TooltipProvider>
);
