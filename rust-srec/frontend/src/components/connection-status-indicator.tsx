import { Button } from '@/components/ui/button';
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from '@/components/ui/tooltip';
import { useDownloadStore, type ConnectionStatus } from '@/store/downloads';
import { useStore } from '@/hooks/use-store';
import { cn } from '@/lib/utils';
import { useLingui } from '@lingui/react';
import { msg } from '@lingui/core/macro';

interface StatusConfig {
  dotColor: string;
  glowColor: string;
  label: { id: string; message?: string };
}

const STATUS_CONFIG: Record<ConnectionStatus, StatusConfig> = {
  connected: {
    dotColor: 'bg-emerald-500',
    glowColor: 'shadow-emerald-500/50',
    label: msg`Connected`,
  },
  connecting: {
    dotColor: 'bg-amber-400',
    glowColor: 'shadow-amber-400/50',
    label: msg`Connecting...`,
  },
  disconnected: {
    dotColor: 'bg-slate-400',
    glowColor: '',
    label: msg`Disconnected`,
  },
  error: {
    dotColor: 'bg-red-500',
    glowColor: 'shadow-red-500/50',
    label: msg`Connection Error`,
  },
};

export function ConnectionStatusIndicator() {
  const { i18n } = useLingui();
  // Use hydration-safe wrapper - returns undefined during SSR
  const connectionStatus =
    useStore(useDownloadStore, (state) => state.connectionStatus) ??
    'disconnected';
  const config = STATUS_CONFIG[connectionStatus];

  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          variant="ghost"
          size="icon"
          className="h-9 w-9 rounded-full relative"
        >
          <span className="relative flex h-2.5 w-2.5">
            {connectionStatus === 'connecting' && (
              <span
                className={cn(
                  'rs-connection-ping absolute inline-flex h-full w-full rounded-full',
                  config.dotColor,
                )}
              />
            )}
            {connectionStatus === 'connected' && (
              <span
                className={cn(
                  'rs-connection-glow absolute inline-flex h-full w-full rounded-full',
                  config.dotColor,
                )}
              />
            )}
            {connectionStatus === 'error' && (
              <span
                className={cn(
                  'rs-connection-error-pulse absolute inline-flex h-full w-full rounded-full',
                  config.dotColor,
                )}
              />
            )}
            <span
              key={connectionStatus}
              className={cn(
                'rs-connection-dot relative inline-flex h-2.5 w-2.5 rounded-full transition-colors duration-300',
                config.dotColor,
                config.glowColor && `shadow-[0_0_8px_2px] ${config.glowColor}`,
              )}
            />
          </span>
        </Button>
      </TooltipTrigger>
      <TooltipContent side="bottom" className="flex items-center gap-2">
        <span
          key={connectionStatus}
          className={cn(
            'rs-tooltip-status-dot inline-block h-2 w-2 rounded-full',
            config.dotColor,
          )}
        />
        <span key={config.label.id} className="rs-tooltip-status-label">
          {i18n._(config.label)}
        </span>
      </TooltipContent>
    </Tooltip>
  );
}
