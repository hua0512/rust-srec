import { useState, type ReactNode } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { Plus } from 'lucide-react';
import { toast } from 'sonner';
import { Button } from '@/components/ui/button';
import {
  deleteCredentialProfile,
  refreshCredentialProfile,
  updateCredentialProfile,
  validateCredentialProfile,
} from '@/server/functions/credential-profiles';
import {
  invalidateCredentialQueries,
  platformAccountsQueryOptions,
  platformCapabilitiesQueryOptions,
} from '@/api/credential-profiles';
import type {
  CredentialLoginTarget,
  CredentialProfileDetail,
} from '@/api/schemas/credential-profiles';
import { AccountListEmpty } from './account-list';
import { AccountRow, type AccountActivity } from './account-row';
import { ListPanel, ListPanelRows } from '@/components/shared/list-panel';
import { CredentialLoginDialog } from './credential-login-dialog';
import {
  DeleteProfileDialog,
  profileReferencesFromConflict,
} from './delete-profile-dialog';
import { ProfileEditorDialog } from './profile-editor-dialog';

/**
 * Manages the platform's accounts: add, edit, validate, refresh, QR login,
 * enable or disable, and delete. Every change is saved immediately, apart from
 * the platform's own selection, `selection`, which sits between the header and
 * the accounts and is saved with the configuration form. Each account's action
 * runs on its own, so a slow check on one row leaves the others usable.
 */
export function PlatformAccountsSection({
  platformId,
  platformName,
  selection,
}: {
  platformId: string;
  platformName?: string;
  selection?: ReactNode;
}) {
  const { i18n } = useLingui();
  const [editor, setEditor] = useState<{
    profile?: CredentialProfileDetail;
    /** Open with credential replacement switched on. */
    replace?: boolean;
  }>();
  const [deleting, setDeleting] = useState<CredentialProfileDetail>();
  /** The QR login in progress and the label of the account it signs in. */
  const [qrLogin, setQrLogin] = useState<{
    target: CredentialLoginTarget;
    label: string;
  }>();
  const queryClient = useQueryClient();
  const accounts = useQuery(platformAccountsQueryOptions(platformId));
  const capabilities = useQuery(platformCapabilitiesQueryOptions(platformId));
  const invalidate = () => invalidateCredentialQueries(queryClient, platformId);
  // What runs on each account, so only that row shows progress.
  const [pending, setPending] = useState<Record<string, AccountActivity>>({});
  const action = useMutation({
    mutationFn: async ({
      type,
      profile,
    }: {
      type: AccountActivity;
      profile: CredentialProfileDetail;
    }) => {
      const { id, version, enabled } = profile.profile;
      if (type === 'delete')
        await deleteCredentialProfile({
          data: { id, expected_version: version },
        });
      else if (type === 'validate')
        await validateCredentialProfile({ data: id });
      else if (type === 'refresh') await refreshCredentialProfile({ data: id });
      else
        await updateCredentialProfile({
          data: { id, expected_version: version, enabled: !enabled },
        });
    },
    onMutate: ({ type, profile }) =>
      setPending((current) => ({ ...current, [profile.profile.id]: type })),
    onSettled: (_data, _error, { profile }) =>
      setPending((current) => {
        const { [profile.profile.id]: _settled, ...rest } = current;
        return rest;
      }),
    onSuccess: invalidate,
    onError: (error: Error, { type, profile }) => {
      // The list was stale: show what the backend says still uses it.
      const references =
        type === 'delete' ? profileReferencesFromConflict(error) : undefined;
      if (references) {
        setDeleting({ ...profile, references });
        void invalidate();
      } else toast.error(error.message);
    },
  });
  const copyId = (id: string) =>
    navigator.clipboard.writeText(id).then(
      () => toast.success(i18n._(msg`Account ID copied`)),
      () => toast.error(i18n._(msg`Could not copy to the clipboard`)),
    );

  const addButton = (variant: 'outline' | 'default') => (
    <Button
      type="button"
      variant={variant}
      size="sm"
      className="h-8 shrink-0 gap-1.5 rounded-lg"
      onClick={() => setEditor({})}
    >
      <Plus className="h-4 w-4" />
      <Trans>Add account</Trans>
    </Button>
  );

  return (
    <>
      <ListPanel
        title={<Trans>Accounts</Trans>}
        description={<Trans>Changes to accounts are saved right away.</Trans>}
        // While the list loads or is empty, its own empty state offers the
        // only add button.
        action={
          (accounts.error || Boolean(accounts.data?.length)) &&
          addButton('outline')
        }
      >
        {selection}
        <ListPanelRows
          query={accounts}
          empty={
            <AccountListEmpty
              help={
                capabilities.data?.qr_login ? (
                  <Trans>
                    Add one by scanning a QR code with the platform&apos;s app
                    or by pasting its cookies.
                  </Trans>
                ) : (
                  <Trans>
                    Add one with its cookies or tokens to record signed in.
                  </Trans>
                )
              }
              action={addButton('default')}
            />
          }
          renderRow={(detail) => (
            <AccountRow
              key={detail.profile.id}
              detail={detail}
              actions={{
                pending: pending[detail.profile.id],
                onAction: (type) => action.mutate({ type, profile: detail }),
                onEdit: (replace) => setEditor({ profile: detail, replace }),
                onDelete: () => setDeleting(detail),
                onCopyId: () => void copyId(detail.profile.id),
                onQrLogin: () =>
                  setQrLogin({
                    target: {
                      type: 'replace',
                      profile_id: detail.profile.id,
                      expected_version: detail.profile.version,
                    },
                    label: detail.profile.label,
                  }),
              }}
            />
          )}
        />
      </ListPanel>
      {editor && (
        <ProfileEditorDialog
          key={editor.profile?.profile.id ?? 'new'}
          platformId={platformId}
          platformName={platformName}
          profile={editor.profile}
          replace={editor.replace}
          onClose={() => setEditor(undefined)}
          onSaved={() => {
            setEditor(undefined);
            void invalidate();
          }}
          onQr={(label, proxyRoute) => {
            setEditor(undefined);
            setQrLogin({
              target: {
                type: 'create',
                platform_id: platformId,
                label,
                // The backend signs in through the platform's route for an
                // account that inherits, so only another choice is sent.
                ...(proxyRoute.kind !== 'inherit'
                  ? { proxy_route: proxyRoute }
                  : {}),
              },
              label,
            });
          }}
        />
      )}
      <DeleteProfileDialog
        detail={deleting}
        onClose={() => setDeleting(undefined)}
        onConfirm={(detail) =>
          action.mutate({ type: 'delete', profile: detail })
        }
      />
      {qrLogin && (
        <CredentialLoginDialog
          target={qrLogin.target}
          accountLabel={qrLogin.label}
          platformName={platformName}
          onClose={() => setQrLogin(undefined)}
          onSuccess={() => void invalidate()}
        />
      )}
    </>
  );
}
