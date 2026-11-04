import { useBlocker } from '@tanstack/react-router';
import { Trans } from '@lingui/react/macro';
import { ConfirmDialog } from '@/components/shared/confirm-dialog';

/**
 * Asks before an in-app navigation drops unsaved edits, and lets the browser
 * ask before a reload or tab close does.
 *
 * Pass `when={isDirty && !isSaving}`: a save that navigates away on success
 * does so while its request is still marked pending, so it is not blocked.
 */
export function UnsavedChangesGuard({ when }: { when: boolean }) {
  const blocker = useBlocker({
    shouldBlockFn: () => when,
    enableBeforeUnload: () => when,
    withResolver: true,
  });

  return (
    <ConfirmDialog
      open={blocker.status === 'blocked'}
      onOpenChange={() => {}}
      title={<Trans>Discard unsaved changes?</Trans>}
      description={
        <Trans>You have unsaved changes. Leave without saving?</Trans>
      }
      confirmLabel={<Trans context="unsaved-changes">Leave</Trans>}
      cancelLabel={<Trans context="unsaved-changes">Stay</Trans>}
      onConfirm={() => blocker.proceed?.()}
      onCancel={() => blocker.reset?.()}
    />
  );
}
