import { Gauge, LayoutTemplate, Pause, Play, Trash2 } from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { msg } from '@lingui/core/macro';

import type { BatchStreamerAction, Template } from '@/api/schemas';
import { Button } from '@/components/ui/button';
import { BatchActionBar } from '@/components/shared/batch-action-bar';
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
    <BatchActionBar
      selectedCount={selectedCount}
      pageCount={pageCount}
      allPageSelected={allPageSelected}
      isPending={isPending}
      onSelectPage={onSelectPage}
      onClearSelection={onClearSelection}
      onExit={onExit}
    >
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
    </BatchActionBar>
  );
}
