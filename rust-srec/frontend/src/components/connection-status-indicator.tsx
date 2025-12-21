import { motion, AnimatePresence } from 'motion/react';
import { Button } from '@/components/ui/button';
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from '@/components/ui/tooltip';
import { useDownloadStore, type ConnectionStatus } from '@/store/downloads';
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
  const connectionStatus = useDownloadStore((state) => state.connectionStatus);
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
            <AnimatePresence mode="wait">
              {/* Ping animation for connecting */}
              {connectionStatus === 'connecting' && (
                <motion.span
                  key="ping"
                  initial={{ scale: 0.8, opacity: 0 }}
                  animate={{ scale: [1, 1.5, 1], opacity: [0.75, 0, 0.75] }}
                  transition={{
                    duration: 1.5,
                    repeat: Infinity,
                    ease: 'easeInOut',
                  }}
                  className={cn(
                    'absolute inline-flex h-full w-full rounded-full',
                    config.dotColor,
                  )}
                />
              )}

              {/* Glow ring for connected */}
              {connectionStatus === 'connected' && (
                <motion.span
                  key="glow"
                  initial={{ scale: 0.8, opacity: 0 }}
                  animate={{ scale: 1.5, opacity: 0.4 }}
                  transition={{ duration: 0.3, ease: 'easeOut' }}
                  className={cn(
                    'absolute inline-flex h-full w-full rounded-full',
                    config.dotColor,
                  )}
                />
              )}

              {/* Error pulse */}
              {connectionStatus === 'error' && (
                <motion.span
                  key="error-pulse"
                  initial={{ scale: 1, opacity: 0.5 }}
                  animate={{ scale: [1, 1.3, 1], opacity: [0.5, 0.2, 0.5] }}
                  transition={{
                    duration: 2,
                    repeat: Infinity,
                    ease: 'easeInOut',
                  }}
                  className={cn(
                    'absolute inline-flex h-full w-full rounded-full',
                    config.dotColor,
                  )}
                />
              )}
            </AnimatePresence>

            {/* Core dot with color transition */}
            <motion.span
              key={connectionStatus}
              initial={{ scale: 0.5, opacity: 0 }}
              animate={{ scale: 1, opacity: 1 }}
              transition={{ duration: 0.2, ease: 'easeOut' }}
              className={cn(
                'relative inline-flex rounded-full h-2.5 w-2.5 transition-colors duration-300',
                config.dotColor,
                config.glowColor && `shadow-[0_0_8px_2px] ${config.glowColor}`,
              )}
            />
          </span>
        </Button>
      </TooltipTrigger>
      <TooltipContent side="bottom" className="flex items-center gap-2">
        <motion.span
          key={connectionStatus}
          initial={{ scale: 0.8 }}
          animate={{ scale: 1 }}
          className={cn('inline-block w-2 h-2 rounded-full', config.dotColor)}
        />
        <motion.span
          key={config.label.id}
          initial={{ opacity: 0, x: -5 }}
          animate={{ opacity: 1, x: 0 }}
          transition={{ duration: 0.2 }}
        >
          {i18n._(config.label)}
        </motion.span>
      </TooltipContent>
    </Tooltip>
  );
}
