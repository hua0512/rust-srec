import { useState } from 'react';
import { Trash2 } from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { msg, plural } from '@lingui/core/macro';

import { Button } from '@/components/ui/button';
import { BatchActionBar } from '@/components/shared/batch-action-bar';
import { ConfirmDialog } from '@/components/shared/confirm-dialog';
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from '@/components/ui/tooltip';

interface SessionBatchActionBarProps {
  selectedCount: number;
  pageCount: number;
  allPageSelected: boolean;
  isPending: boolean;
  onSelectPage: () => void;
  onClearSelection: () => void;
  onDelete: () => void;
  onExit: () => void;
}

export function SessionBatchActionBar({
  selectedCount,
  pageCount,
  allPageSelected,
  isPending,
  onSelectPage,
  onClearSelection,
  onDelete,
  onExit,
}: SessionBatchActionBarProps) {
  const { i18n } = useLingui();
  const [confirmOpen, setConfirmOpen] = useState(false);

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
            disabled={selectedCount === 0 || isPending}
            onClick={() => setConfirmOpen(true)}
            className="rounded-full text-destructive hover:bg-destructive/10 hover:text-destructive"
            aria-label={i18n._(msg`Delete selected`)}
          >
            <Trash2 />
            <span className="hidden sm:inline">
              <Trans>Delete</Trans>
            </span>
          </Button>
        </TooltipTrigger>
        <TooltipContent side="top">
          <Trans>Delete selected</Trans>
        </TooltipContent>
      </Tooltip>

      <ConfirmDialog
        open={confirmOpen}
        onOpenChange={setConfirmOpen}
        title={<Trans>Delete sessions</Trans>}
        description={i18n._(
          msg`${plural(selectedCount, {
            one: 'Are you sure you want to delete # session? This action cannot be undone.',
            other:
              'Are you sure you want to delete # sessions? This action cannot be undone.',
          })}`,
        )}
        onConfirm={onDelete}
      />
    </BatchActionBar>
  );
}
