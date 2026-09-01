import { useState } from 'react';
import { Trans } from '@lingui/react/macro';
import { plural, t } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog';
import { Checkbox } from '@/components/ui/checkbox';
import { Label } from '@/components/ui/label';

interface DeleteOutputsDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** How many outputs the confirmation covers. */
  count: number;
  onConfirm: (deleteFile: boolean) => void;
}

/**
 * Confirmation for removing media outputs, shared by the per-card menu and the
 * batch action bar.
 *
 * `deleteFile` starts off so the default action only removes the database
 * record, matching how deleting a session leaves its files in place. It resets
 * each time the dialog closes so a previous confirmation cannot silently arm
 * the next one.
 */
export function DeleteOutputsDialog({
  open,
  onOpenChange,
  count,
  onConfirm,
}: DeleteOutputsDialogProps) {
  const { i18n } = useLingui();
  const [deleteFile, setDeleteFile] = useState(false);

  const handleOpenChange = (next: boolean) => {
    if (!next) setDeleteFile(false);
    onOpenChange(next);
  };

  const countLabel = t(
    i18n,
  )`${plural(count, { one: '# output', other: '# outputs' })}`;

  return (
    <AlertDialog open={open} onOpenChange={handleOpenChange}>
      <AlertDialogContent
        className="rounded-2xl"
        onClick={(event) => event.stopPropagation()}
      >
        <AlertDialogHeader>
          <AlertDialogTitle>
            <Trans>Delete outputs?</Trans>
          </AlertDialogTitle>
          <AlertDialogDescription>
            <Trans>
              This removes {countLabel} from the recording's file list. This
              action cannot be undone.
            </Trans>
          </AlertDialogDescription>
        </AlertDialogHeader>

        <div className="flex items-start gap-3 rounded-xl border border-destructive/20 bg-destructive/5 p-3">
          <Checkbox
            id="delete-output-files"
            checked={deleteFile}
            onCheckedChange={(checked) => setDeleteFile(checked === true)}
            className="mt-0.5"
          />
          <div className="space-y-1">
            <Label
              htmlFor="delete-output-files"
              className="text-sm font-medium cursor-pointer"
            >
              <Trans>Also delete files from disk</Trans>
            </Label>
            <p className="text-xs text-muted-foreground">
              <Trans>
                Leave this off to only remove the records and keep the files
                where they are.
              </Trans>
            </p>
          </div>
        </div>

        <AlertDialogFooter>
          <AlertDialogCancel className="rounded-full">
            <Trans>Cancel</Trans>
          </AlertDialogCancel>
          <AlertDialogAction
            onClick={() => onConfirm(deleteFile)}
            className="rounded-full bg-destructive text-white hover:bg-destructive/90"
          >
            {deleteFile ? (
              <Trans>Delete with files</Trans>
            ) : (
              <Trans>Delete</Trans>
            )}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
