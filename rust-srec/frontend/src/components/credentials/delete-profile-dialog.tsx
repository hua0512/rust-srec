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
import {
  ProfileReferencesSchema,
  type CredentialProfileDetail,
  type ProfileReferences,
} from '@/api/schemas/credential-profiles';
import { referencesFromConflict } from '@/lib/api-error';
import { DIALOG_NAME_LIMIT } from '@/components/shared/list-panel';
import { AccountReferences } from './account-usage';

/**
 * The references a refused deletion lists, when the backend refused it because
 * the account is still in use.
 */
export function profileReferencesFromConflict(
  error: unknown,
): ProfileReferences | undefined {
  return referencesFromConflict(
    error,
    'CREDENTIAL_PROFILE_REFERENCED',
    ProfileReferencesSchema,
  );
}

/**
 * Confirms a deletion, or explains what keeps an account in use from being
 * deleted. The explanation has nothing to confirm, so it is a plain dialog
 * with a close button rather than an alert dialog.
 */
export function DeleteProfileDialog({
  detail,
  onClose,
  onConfirm,
}: {
  detail: CredentialProfileDetail | undefined;
  onClose: () => void;
  onConfirm: (detail: CredentialProfileDetail) => void;
}) {
  const { i18n } = useLingui();
  const label = detail?.profile.label ?? '';
  const selections = detail?.references.selections.length ?? 0;
  const sessions = detail?.references.recordings.length ?? 0;
  const blocked = sessions + selections > 0;
  const onOpenChange = (open: boolean) => {
    if (!open) onClose();
  };
  if (blocked)
    return (
      <Dialog open={Boolean(detail)} onOpenChange={onOpenChange}>
        <DialogContent className="border-border/50">
          <DialogHeader>
            <DialogTitle>
              <Trans>This account is in use</Trans>
            </DialogTitle>
            <DialogDescription asChild>
              <div className="space-y-2">
                <p>
                  <Trans>
                    &quot;{label}&quot; cannot be deleted while it is selected
                    or recording.
                  </Trans>
                </p>
                <ul className="list-disc pl-5">
                  {selections > 0 && (
                    <li>
                      {t(i18n)`${plural(selections, {
                        one: 'Remove it from the account selection of # configuration.',
                        other:
                          'Remove it from the account selection of # configurations.',
                      })}`}
                    </li>
                  )}
                  {sessions > 0 && (
                    <li>
                      {t(i18n)`${plural(sessions, {
                        one: 'Wait for the active recording using it to end.',
                        other:
                          'Wait for the # active recordings using it to end.',
                      })}`}
                    </li>
                  )}
                </ul>
                {detail && (
                  <AccountReferences
                    references={detail.references}
                    limit={DIALOG_NAME_LIMIT}
                    className="space-y-1 rounded-lg border border-border/60 bg-muted/30 px-3 py-2 text-xs text-muted-foreground"
                  />
                )}
                <p>
                  <Trans>
                    To stop using it right away, disable it instead.
                  </Trans>
                </p>
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
    <AlertDialog open={Boolean(detail)} onOpenChange={onOpenChange}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>
            <Trans>Delete account</Trans>
          </AlertDialogTitle>
          <AlertDialogDescription>
            <Trans>
              Delete &quot;{label}&quot; and its saved credentials? This cannot
              be undone.
            </Trans>
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>
            <Trans>Cancel</Trans>
          </AlertDialogCancel>
          {detail && (
            <AlertDialogAction
              onClick={() => onConfirm(detail)}
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
