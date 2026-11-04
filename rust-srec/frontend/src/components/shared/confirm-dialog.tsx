import type { ReactNode } from 'react';
import { Trans } from '@lingui/react/macro';
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

interface ConfirmDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: ReactNode;
  description: ReactNode;
  /** Label of the confirming button; defaults to "Delete". */
  confirmLabel?: ReactNode;
  /** Label of the dismissing button; defaults to "Cancel". */
  cancelLabel?: ReactNode;
  onConfirm: () => void;
  /** Called when the dialog is dismissed without confirming. */
  onCancel?: () => void;
}

/** Asks before a destructive action; the confirming button is styled as destructive. */
export function ConfirmDialog({
  open,
  onOpenChange,
  title,
  description,
  confirmLabel,
  cancelLabel,
  onConfirm,
  onCancel,
}: ConfirmDialogProps) {
  return (
    <AlertDialog
      open={open}
      onOpenChange={(next) => {
        if (!next) onCancel?.();
        onOpenChange(next);
      }}
    >
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>{title}</AlertDialogTitle>
          <AlertDialogDescription>{description}</AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>
            {cancelLabel ?? <Trans>Cancel</Trans>}
          </AlertDialogCancel>
          <AlertDialogAction
            onClick={(event) => {
              // Confirming closes the dialog too; it must not also count as a
              // cancel through `onOpenChange`.
              event.preventDefault();
              onConfirm();
              onOpenChange(false);
            }}
            className="bg-destructive text-white hover:bg-destructive/90"
          >
            {confirmLabel ?? <Trans>Delete</Trans>}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
