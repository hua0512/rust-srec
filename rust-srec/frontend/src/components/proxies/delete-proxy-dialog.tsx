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
import { Button } from '@/components/ui/button';
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { DIALOG_NAME_LIMIT } from '@/components/shared/list-panel';
import type { ProxyReferences, SavedProxy } from '@/api/schemas/proxies';
import { referenceCount } from './proxy-errors';
import { ProxyReferencesList } from './proxy-references';

/**
 * Confirms deleting a proxy, or, when settings still use it, lists them. The
 * list has nothing to confirm, so it is a plain dialog with a close button.
 */
export function DeleteProxyDialog({
  target,
  onClose,
  onConfirm,
}: {
  target: { proxy: SavedProxy; references?: ProxyReferences } | undefined;
  onClose: () => void;
  onConfirm: (proxy: SavedProxy) => void;
}) {
  const name = target?.proxy.name ?? '';
  const references = target?.references;
  const onOpenChange = (open: boolean) => {
    if (!open) onClose();
  };
  if (references && referenceCount(references) > 0)
    return (
      <Dialog open={Boolean(target)} onOpenChange={onOpenChange}>
        <DialogContent className="border-border/50">
          <DialogHeader>
            <DialogTitle>
              <Trans>This proxy is in use</Trans>
            </DialogTitle>
            <DialogDescription asChild>
              <div className="space-y-3">
                <p>
                  <Trans>
                    &quot;{name}&quot; cannot be deleted while these settings
                    use it. Choose another proxy in each of them first.
                  </Trans>
                </p>
                <ProxyReferencesList
                  references={references}
                  limit={DIALOG_NAME_LIMIT}
                  className="rounded-lg border border-border/60 bg-muted/30 px-3 py-2 text-xs"
                />
                {references.templates.some(
                  (template) => template.being_removed,
                ) && (
                  <p className="text-xs">
                    <Trans>
                      A template being removed lets go of the proxy once the
                      recordings using it finish.
                    </Trans>
                  </p>
                )}
              </div>
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <DialogClose asChild>
              <Button type="button" variant="outline">
                <Trans>Close</Trans>
              </Button>
            </DialogClose>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    );
  return (
    <AlertDialog open={Boolean(target)} onOpenChange={onOpenChange}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>
            <Trans>Delete proxy</Trans>
          </AlertDialogTitle>
          <AlertDialogDescription>
            <Trans>
              Delete &quot;{name}&quot; and its saved login? This cannot be
              undone.
            </Trans>
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>
            <Trans>Cancel</Trans>
          </AlertDialogCancel>
          {target && (
            <AlertDialogAction
              onClick={() => onConfirm(target.proxy)}
              className="bg-destructive text-white hover:bg-destructive/90"
            >
              <Trans>Delete</Trans>
            </AlertDialogAction>
          )}
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
