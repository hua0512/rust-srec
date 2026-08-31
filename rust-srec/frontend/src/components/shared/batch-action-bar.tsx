import type { ReactNode } from 'react';
import { motion } from 'motion/react';
import { CheckCheck, Eraser, Loader2, X } from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { msg } from '@lingui/core/macro';

import { Button } from '@/components/ui/button';
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from '@/components/ui/tooltip';

interface BatchActionBarProps {
  selectedCount: number;
  /** Number of rows on the current page, used to disable select-page when empty. */
  pageCount: number;
  allPageSelected: boolean;
  isPending: boolean;
  onSelectPage: () => void;
  onClearSelection: () => void;
  onExit: () => void;
  /** Per-page action controls, rendered between the selection and exit controls. */
  children: ReactNode;
}

/**
 * Floating pill holding a list page's batch controls: the selection count, the
 * select-page / clear-selection buttons, the caller's action buttons, and exit.
 *
 * Callers render it inside an `AnimatePresence` gated on their selection mode so
 * the enter/exit transitions run.
 */
export function BatchActionBar({
  selectedCount,
  pageCount,
  allPageSelected,
  isPending,
  onSelectPage,
  onClearSelection,
  onExit,
  children,
}: BatchActionBarProps) {
  const { i18n } = useLingui();

  return (
    <motion.div
      initial={{ opacity: 0, y: 24, scale: 0.96 }}
      animate={{ opacity: 1, y: 0, scale: 1 }}
      exit={{ opacity: 0, y: 24, scale: 0.96 }}
      className="fixed inset-x-3 bottom-[max(0.75rem,env(safe-area-inset-bottom))] z-50 mx-auto flex w-fit max-w-[calc(100vw-1.5rem)] items-center gap-1 overflow-x-auto rounded-full border border-border/60 bg-background/95 p-1.5 shadow-2xl backdrop-blur-xl no-scrollbar sm:inset-x-auto sm:left-1/2 sm:-translate-x-1/2"
    >
      <div className="flex h-8 shrink-0 items-center gap-2 rounded-full bg-primary/10 px-3 text-primary ring-1 ring-primary/20">
        <span className="text-sm font-bold tabular-nums">{selectedCount}</span>
        <span className="hidden text-xs font-medium sm:inline">
          <Trans>selected</Trans>
        </span>
      </div>

      <Tooltip>
        <TooltipTrigger asChild>
          <Button
            variant="ghost"
            size="icon-sm"
            disabled={pageCount === 0 || allPageSelected || isPending}
            onClick={onSelectPage}
            className="rounded-full"
            aria-label={i18n._(msg`Select current page`)}
          >
            <CheckCheck />
          </Button>
        </TooltipTrigger>
        <TooltipContent side="top">
          <Trans>Select current page</Trans>
        </TooltipContent>
      </Tooltip>

      <Tooltip>
        <TooltipTrigger asChild>
          <Button
            variant="ghost"
            size="icon-sm"
            disabled={selectedCount === 0 || isPending}
            onClick={onClearSelection}
            className="rounded-full"
            aria-label={i18n._(msg`Clear selection`)}
          >
            <Eraser />
          </Button>
        </TooltipTrigger>
        <TooltipContent side="top">
          <Trans>Clear selection</Trans>
        </TooltipContent>
      </Tooltip>

      <div className="mx-1 h-5 w-px shrink-0 bg-border" />

      {children}

      <div className="mx-1 h-5 w-px shrink-0 bg-border" />

      <Tooltip>
        <TooltipTrigger asChild>
          <Button
            variant="ghost"
            size="icon-sm"
            disabled={isPending}
            onClick={onExit}
            className="rounded-full"
            aria-label={i18n._(msg`Exit selection mode`)}
          >
            {isPending ? <Loader2 className="animate-spin" /> : <X />}
          </Button>
        </TooltipTrigger>
        <TooltipContent side="top">
          <Trans>Exit selection mode</Trans>
        </TooltipContent>
      </Tooltip>
    </motion.div>
  );
}
