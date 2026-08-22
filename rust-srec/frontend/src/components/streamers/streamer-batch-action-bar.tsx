import { motion } from 'motion/react';
import {
  CheckCheck,
  Eraser,
  Gauge,
  LayoutTemplate,
  Loader2,
  Pause,
  Play,
  Trash2,
  X,
} from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { msg } from '@lingui/core/macro';

import type { BatchStreamerAction, Template } from '@/api/schemas';
import { Button } from '@/components/ui/button';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from '@/components/ui/alert-dialog';
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from '@/components/ui/tooltip';

interface StreamerBatchActionBarProps {
  selectedCount: number;
  pageCount: number;
  allPageSelected: boolean;
  templates: Template[];
  isPending: boolean;
  onSelectPage: () => void;
  onClearSelection: () => void;
  onAction: (action: BatchStreamerAction) => void;
  onExit: () => void;
}

export function StreamerBatchActionBar({
  selectedCount,
  pageCount,
  allPageSelected,
  templates,
  isPending,
  onSelectPage,
  onClearSelection,
  onAction,
  onExit,
}: StreamerBatchActionBarProps) {
  const { i18n } = useLingui();
  const commandsDisabled = selectedCount === 0 || isPending;

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

      <Tooltip>
        <TooltipTrigger asChild>
          <Button
            variant="ghost"
            size="sm"
            disabled={commandsDisabled}
            aria-label={i18n._(msg`Enable selected`)}
            onClick={() => onAction({ type: 'set_enabled', enabled: true })}
            className="rounded-full text-emerald-600 hover:bg-emerald-500/10 hover:text-emerald-700 dark:text-emerald-400"
          >
            <Play />
            <span className="hidden sm:inline">
              <Trans>Enable</Trans>
            </span>
          </Button>
        </TooltipTrigger>
        <TooltipContent side="top">
          <Trans>Enable selected</Trans>
        </TooltipContent>
      </Tooltip>

      <Tooltip>
        <TooltipTrigger asChild>
          <Button
            variant="ghost"
            size="sm"
            disabled={commandsDisabled}
            aria-label={i18n._(msg`Disable selected`)}
            onClick={() => onAction({ type: 'set_enabled', enabled: false })}
            className="rounded-full text-amber-600 hover:bg-amber-500/10 hover:text-amber-700 dark:text-amber-400"
          >
            <Pause />
            <span className="hidden sm:inline">
              <Trans>Disable</Trans>
            </span>
          </Button>
        </TooltipTrigger>
        <TooltipContent side="top">
          <Trans>Disable selected</Trans>
        </TooltipContent>
      </Tooltip>

      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            variant="ghost"
            size="sm"
            disabled={commandsDisabled}
            aria-label={i18n._(msg`Assign template`)}
            className="rounded-full"
          >
            <LayoutTemplate />
            <span className="hidden sm:inline">
              <Trans>Template</Trans>
            </span>
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="center" className="max-h-72 w-56">
          <DropdownMenuLabel>
            <Trans>Assign template</Trans>
          </DropdownMenuLabel>
          <DropdownMenuItem
            onClick={() =>
              onAction({ type: 'set_template', template_id: null })
            }
          >
            <Trans>No template assigned</Trans>
          </DropdownMenuItem>
          {templates.length > 0 && <DropdownMenuSeparator />}
          {templates.map((template) => (
            <DropdownMenuItem
              key={template.id}
              onClick={() =>
                onAction({
                  type: 'set_template',
                  template_id: template.id,
                })
              }
            >
              <span className="truncate">{template.name}</span>
            </DropdownMenuItem>
          ))}
        </DropdownMenuContent>
      </DropdownMenu>

      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            variant="ghost"
            size="sm"
            disabled={commandsDisabled}
            aria-label={i18n._(msg`Set priority`)}
            className="rounded-full"
          >
            <Gauge />
            <span className="hidden sm:inline">
              <Trans>Priority</Trans>
            </span>
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="center" className="w-44">
          <DropdownMenuLabel>
            <Trans>Set priority</Trans>
          </DropdownMenuLabel>
          {(['HIGH', 'NORMAL', 'LOW'] as const).map((priority) => (
            <DropdownMenuItem
              key={priority}
              onClick={() => onAction({ type: 'set_priority', priority })}
            >
              {priority === 'HIGH' ? (
                <Trans>High</Trans>
              ) : priority === 'NORMAL' ? (
                <Trans>Normal</Trans>
              ) : (
                <Trans>Low</Trans>
              )}
            </DropdownMenuItem>
          ))}
        </DropdownMenuContent>
      </DropdownMenu>

      <AlertDialog>
        <Tooltip>
          <TooltipTrigger asChild>
            <AlertDialogTrigger asChild>
              <Button
                variant="ghost"
                size="icon-sm"
                disabled={commandsDisabled}
                className="rounded-full text-destructive hover:bg-destructive/10 hover:text-destructive"
                aria-label={i18n._(msg`Delete selected`)}
              >
                <Trash2 />
              </Button>
            </AlertDialogTrigger>
          </TooltipTrigger>
          <TooltipContent side="top">
            <Trans>Delete selected</Trans>
          </TooltipContent>
        </Tooltip>
        <AlertDialogContent className="rounded-2xl">
          <AlertDialogHeader>
            <AlertDialogTitle>
              <Trans>Delete selected streamers?</Trans>
            </AlertDialogTitle>
            <AlertDialogDescription>
              <Trans>
                This will permanently delete {selectedCount} streamers. This
                action cannot be undone.
              </Trans>
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel className="rounded-full">
              <Trans>Cancel</Trans>
            </AlertDialogCancel>
            <AlertDialogAction
              onClick={() => onAction({ type: 'delete' })}
              className="rounded-full bg-destructive text-white hover:bg-destructive/90"
            >
              <Trans>Delete</Trans>
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

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
