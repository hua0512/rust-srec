import { useState } from 'react';
import { Trash2 } from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { msg } from '@lingui/core/macro';

import { Button } from '@/components/ui/button';
import { BatchActionBar } from '@/components/shared/batch-action-bar';
import { DeleteOutputsDialog } from '@/components/pipeline/outputs/delete-outputs-dialog';
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from '@/components/ui/tooltip';

interface OutputBatchActionBarProps {
  selectedCount: number;
  pageCount: number;
  allPageSelected: boolean;
  isPending: boolean;
  onSelectPage: () => void;
  onClearSelection: () => void;
  onDelete: (deleteFile: boolean) => void;
  onExit: () => void;
}

export function OutputBatchActionBar({
  selectedCount,
  pageCount,
  allPageSelected,
  isPending,
  onSelectPage,
  onClearSelection,
  onDelete,
  onExit,
}: OutputBatchActionBarProps) {
  const { i18n } = useLingui();
  const [deleteOpen, setDeleteOpen] = useState(false);

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
            onClick={() => setDeleteOpen(true)}
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

      <DeleteOutputsDialog
        open={deleteOpen}
        onOpenChange={setDeleteOpen}
        count={selectedCount}
        onConfirm={(deleteFile) => {
          setDeleteOpen(false);
          onDelete(deleteFile);
        }}
      />
    </BatchActionBar>
  );
}
