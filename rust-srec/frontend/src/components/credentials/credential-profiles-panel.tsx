import { useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Trans } from '@lingui/react/macro';
import { toast } from 'sonner';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { Textarea } from '@/components/ui/textarea';
import { Badge } from '@/components/ui/badge';
import { Switch } from '@/components/ui/switch';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import {
  createCredentialProfile,
  updateCredentialProfile,
  deleteCredentialProfile,
  listCredentialProfiles,
  refreshCredentialProfile,
  validateCredentialProfile,
  getEffectiveCredentialSelection,
  previewCredentialConversion,
  convertLegacyCredentials,
} from '@/server/functions/credential-profiles';
import type {
  CredentialLoginTarget,
  CredentialOwner,
  CredentialProfileDetail,
  CredentialSelection,
} from '@/api/schemas/credential-profiles';
import { CredentialSelectionEditor } from './credential-selection-editor';
import { CredentialLoginDialog } from './credential-login-dialog';

function UnavailableReason({ reason }: { reason: string }) {
  switch (reason) {
    case 'cooling_down':
      return (
        <Trans>
          Every selected account is cooling down after being throttled. Checks
          resume automatically.
        </Trans>
      );
    case 'login_required':
      return (
        <Trans>
          Every usable account needs a new login. Log in again or replace an
          account&apos;s cookies.
        </Trans>
      );
    case 'profiles_disabled':
      return (
        <Trans>
          Every selected account is disabled. Enable one or change the
          selection.
        </Trans>
      );
    case 'bound_profile_unavailable':
      return (
        <Trans>
          The account pinned to the active recording is unavailable.
        </Trans>
      );
    case 'binding_policy_changed':
      return (
        <Trans>
          The selection changed during the active recording. Its next recovery
          uses the new selection.
        </Trans>
      );
    default:
      return (
        <Trans>
          Authentication is temporarily unavailable. Check the account status
          and retry.
        </Trans>
      );
  }
}

function ownerId(owner: CredentialOwner) {
  return owner.type === 'platform'
    ? owner.platform_id
    : owner.type === 'template'
      ? owner.template_id
      : owner.streamer_id;
}

export function CredentialProfilesPanel({
  owner,
  platformId,
  platformName,
  selection,
  onSelectionChange,
  onConverted,
}: {
  owner: CredentialOwner;
  platformId: string;
  platformName: string;
  selection?: CredentialSelection;
  onSelectionChange: (selection: CredentialSelection) => void;
  onConverted: (selection: CredentialSelection) => void;
}) {
  const client = useQueryClient();
  const [editor, setEditor] = useState<{
    profile?: CredentialProfileDetail;
    replace: boolean;
  }>();
  const [qrTarget, setQrTarget] = useState<CredentialLoginTarget>();
  const [converting, setConverting] = useState(false);
  const queryKey = [
    'credential-profiles',
    owner.type,
    ownerId(owner),
    platformId,
  ];
  const profiles = useQuery({
    queryKey,
    queryFn: () =>
      listCredentialProfiles({
        data: {
          scope_type: owner.type,
          scope_id: ownerId(owner),
          platform_id: platformId,
        },
      }),
  });
  const effective = useQuery({
    queryKey: [...queryKey, 'effective'],
    queryFn: () =>
      getEffectiveCredentialSelection({
        data: {
          scope_type: owner.type,
          scope_id: ownerId(owner),
          platform_id: platformId,
        },
      }),
  });
  const invalidate = () =>
    client.invalidateQueries({ queryKey: ['credential-profiles'] });
  const action = useMutation({
    mutationFn: async ({
      type,
      profile,
    }: {
      type: 'delete' | 'validate' | 'refresh' | 'toggle';
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
    onSuccess: invalidate,
    onError: (error: Error) => toast.error(error.message),
  });
  return (
    <div className="space-y-5">
      {effective.data?.active_binding?.identity.profile_id && (
        <p className="text-sm">
          <Trans>Account pinned to the active recording</Trans>:{' '}
          {profiles.data?.find(
            (entry) =>
              entry.profile.id ===
              effective.data?.active_binding?.identity.profile_id,
          )?.profile.label ?? effective.data.active_binding.identity.profile_id}
        </p>
      )}
      {effective.data?.resolved && (
        <p className="text-sm">
          <Trans>Effective policy owner</Trans>:{' '}
          {effective.data.resolved.owner.type} ·{' '}
          <Trans>Candidate accounts</Trans>: {effective.data.candidates.length}
        </p>
      )}
      {effective.data?.unavailable_reason && (
        <p role="status" className="text-sm text-amber-600">
          <UnavailableReason reason={effective.data.unavailable_reason} />
          {effective.data.unavailable_retry_at != null && (
            <>
              {' '}
              <Trans>Cooldown until</Trans>:{' '}
              {new Date(effective.data.unavailable_retry_at).toLocaleString()}
            </>
          )}
        </p>
      )}
      <CredentialSelectionEditor
        value={selection}
        onChange={onSelectionChange}
        profiles={profiles.data?.map((detail) => detail.profile) ?? []}
      />
      {!selection && (
        <Button
          type="button"
          variant="outline"
          onClick={() => setConverting(true)}
        >
          <Trans>Convert legacy credentials</Trans>
        </Button>
      )}
      <div className="flex items-center justify-between">
        <h3 className="font-medium">
          <Trans>Account profiles</Trans>
        </h3>
        <Button
          type="button"
          variant="outline"
          onClick={() => setEditor({ replace: true })}
        >
          <Trans>Add profile</Trans>
        </Button>
      </div>
      <p className="text-xs text-muted-foreground">
        <Trans>
          Profile changes are saved immediately. Adding an account does not
          change the saved selection.
        </Trans>
      </p>
      {profiles.error && <p role="alert">{profiles.error.message}</p>}
      {profiles.data?.map((detail) => {
        const profile = detail.profile;
        const inherited =
          JSON.stringify(profile.owner) !== JSON.stringify(owner);
        const active = detail.references.filter((reference) =>
          reference.startsWith('session:'),
        );
        return (
          <div key={profile.id} className="space-y-2 rounded-lg border p-3">
            <div className="flex flex-wrap items-center gap-2">
              <strong>{profile.label}</strong>
              <Badge variant="secondary">
                {profile.owner.type}: {ownerId(profile.owner)}
              </Badge>
              {inherited && (
                <Badge>
                  <Trans>Inherited</Trans>
                </Badge>
              )}
              <Badge variant="outline">
                {detail.health?.validity ?? 'unknown'}
              </Badge>
            </div>
            <p className="text-xs text-muted-foreground break-all">
              <Trans>Account profile ID</Trans>: <code>{profile.id}</code>
            </p>
            {!profile.enabled && (
              <p className="text-sm text-amber-600">
                <Trans>
                  Disabled accounts are unavailable for new operations.
                </Trans>
              </p>
            )}
            {detail.health?.cooldown_until != null &&
              detail.health.cooldown_until > Date.now() && (
                <p className="text-sm">
                  <Trans>Cooldown until</Trans>{' '}
                  {new Date(detail.health.cooldown_until).toLocaleString()}
                </p>
              )}
            {active.length > 0 && (
              <p className="text-xs">
                <Trans>Used by active recordings</Trans>: {active.length}
              </p>
            )}
            <div className="flex flex-wrap gap-2">
              <Button
                type="button"
                size="sm"
                variant="outline"
                disabled={
                  action.isPending ||
                  !profile.enabled ||
                  !detail.capabilities.validate
                }
                onClick={() =>
                  action.mutate({ type: 'validate', profile: detail })
                }
              >
                <Trans>Validate</Trans>
              </Button>
              <Button
                type="button"
                size="sm"
                variant="outline"
                disabled={
                  action.isPending ||
                  !profile.enabled ||
                  !detail.capabilities.refresh
                }
                onClick={() =>
                  action.mutate({ type: 'refresh', profile: detail })
                }
              >
                <Trans>Refresh</Trans>
              </Button>
              {!detail.capabilities.validate && (
                <span className="text-xs text-muted-foreground">
                  <Trans>Validation is not supported by this platform.</Trans>
                </span>
              )}
              {!inherited && (
                <>
                  <Button
                    type="button"
                    size="sm"
                    variant="outline"
                    onClick={() =>
                      setEditor({ profile: detail, replace: false })
                    }
                  >
                    <Trans>Edit label</Trans>
                  </Button>
                  <Button
                    type="button"
                    size="sm"
                    variant="outline"
                    onClick={() =>
                      setEditor({ profile: detail, replace: true })
                    }
                  >
                    <Trans>Replace credentials</Trans>
                  </Button>
                  <Button
                    type="button"
                    size="sm"
                    variant="outline"
                    disabled={action.isPending}
                    onClick={() =>
                      action.mutate({ type: 'toggle', profile: detail })
                    }
                  >
                    {profile.enabled ? (
                      <Trans>Disable</Trans>
                    ) : (
                      <Trans>Enable</Trans>
                    )}
                  </Button>
                  <Button
                    type="button"
                    size="sm"
                    variant="destructive"
                    disabled={action.isPending || detail.references.length > 0}
                    onClick={() =>
                      action.mutate({ type: 'delete', profile: detail })
                    }
                  >
                    <Trans>Delete</Trans>
                  </Button>
                  {detail.capabilities.qr_login && (
                    <Button
                      type="button"
                      size="sm"
                      variant="outline"
                      onClick={() =>
                        setQrTarget({
                          type: 'replace',
                          profile_id: profile.id,
                          expected_version: profile.version,
                        })
                      }
                    >
                      <Trans>QR Login</Trans>
                    </Button>
                  )}
                </>
              )}
            </div>
          </div>
        );
      })}
      {editor && (
        <ProfileEditor
          key={`${editor.profile?.profile.id ?? 'new'}-${editor.replace}`}
          owner={owner}
          platformId={platformId}
          platformName={platformName}
          {...editor}
          onClose={() => setEditor(undefined)}
          onSaved={() => {
            setEditor(undefined);
            void invalidate();
          }}
          onQr={(label) => {
            setEditor(undefined);
            setQrTarget({
              type: 'create',
              owner,
              platform_id: platformId,
              label,
            });
          }}
        />
      )}
      {qrTarget && (
        <CredentialLoginDialog
          target={qrTarget}
          onClose={() => setQrTarget(undefined)}
          onSuccess={() => {
            void invalidate();
          }}
        />
      )}
      {converting && (
        <ConversionDialog
          owner={owner}
          platformId={platformId}
          onClose={() => setConverting(false)}
          onSuccess={(value) => {
            onConverted(value);
            setConverting(false);
            void invalidate();
          }}
        />
      )}
    </div>
  );
}

function ConversionDialog({
  owner,
  platformId,
  onClose,
  onSuccess,
}: {
  owner: CredentialOwner;
  platformId: string;
  onClose: () => void;
  onSuccess: (selection: CredentialSelection) => void;
}) {
  const [label, setLabel] = useState('');
  const [source, setSource] = useState<
    'effective' | 'refresh_source' | 'none'
  >();
  const preview = useQuery({
    queryKey: ['credential-conversion', owner.type, ownerId(owner), platformId],
    queryFn: () =>
      previewCredentialConversion({
        data: {
          scope_type: owner.type,
          scope_id: ownerId(owner),
          platform_id: platformId,
        },
      }),
    staleTime: 0,
    gcTime: 0,
  });
  const convert = useMutation({
    mutationFn: async () => {
      if (!source || !preview.data)
        throw new Error('Choose the credential source');
      return convertLegacyCredentials({
        data: {
          owner,
          platform_id: platformId,
          label: label.trim() || 'Converted account',
          source,
          expected_fingerprint: preview.data.fingerprint,
        },
      });
    },
    onSuccess: (result) => onSuccess(result.selection),
  });
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
    >
      <DialogContent>
        <DialogHeader>
          <DialogTitle>
            <Trans>Convert legacy credentials</Trans>
          </DialogTitle>
          <DialogDescription>
            <Trans>
              Conversion immediately saves one profile and a fixed selection, or
              explicitly disables authentication.
            </Trans>
          </DialogDescription>
        </DialogHeader>
        {preview.data && (
          <>
            <p>
              <Trans>Effective cookies source</Trans>:{' '}
              {preview.data.effective_source?.type ?? 'none'}
            </p>
            <p>
              <Trans>Refresh source</Trans>:{' '}
              {preview.data.refresh_source?.type ?? 'none'}
            </p>
            {preview.data.source_choice_required && (
              <p className="text-amber-600">
                <Trans>
                  Effective cookies and the refresh account differ. Choose which
                  material to convert.
                </Trans>
              </p>
            )}
            {preview.data.copies_platform_login && (
              <p className="text-amber-600">
                <Trans>
                  The platform login will be copied into this profile. Later
                  platform password changes will no longer update this account.
                </Trans>
              </p>
            )}
            {preview.data.copies_account_extras &&
              !preview.data.copies_platform_login && (
                <p className="text-amber-600">
                  <Trans>
                    Account tokens or device identity will be copied into this
                    profile. Later parent changes will no longer update this
                    account.
                  </Trans>
                </p>
              )}
            <Label className="grid gap-2">
              <Trans>Label</Trans>
              <Input
                value={label}
                onChange={(event) => setLabel(event.target.value)}
              />
            </Label>
            <div className="grid gap-2">
              <Label>
                <input
                  type="radio"
                  name="conversion-source"
                  disabled={!preview.data.effective_has_material}
                  checked={source === 'effective'}
                  onChange={() => setSource('effective')}
                />{' '}
                <Trans>Use effective cookies</Trans>
              </Label>
              <Label>
                <input
                  type="radio"
                  name="conversion-source"
                  disabled={!preview.data.refresh_has_material}
                  checked={source === 'refresh_source'}
                  onChange={() => setSource('refresh_source')}
                />{' '}
                <Trans>Use refresh account</Trans>
              </Label>
              <Label>
                <input
                  type="radio"
                  name="conversion-source"
                  checked={source === 'none'}
                  onChange={() => setSource('none')}
                />{' '}
                <Trans>No authentication</Trans>
              </Label>
            </div>
          </>
        )}
        {(preview.error || convert.error) && (
          <p role="alert">{preview.error?.message ?? convert.error?.message}</p>
        )}
        <Button
          type="button"
          disabled={!source || !preview.data || convert.isPending}
          onClick={() => convert.mutate()}
        >
          <Trans>Convert and save now</Trans>
        </Button>
      </DialogContent>
    </Dialog>
  );
}

function ProfileEditor({
  owner,
  platformId,
  platformName,
  profile,
  replace,
  onClose,
  onSaved,
  onQr,
}: {
  owner: CredentialOwner;
  platformId: string;
  platformName: string;
  profile?: CredentialProfileDetail;
  replace: boolean;
  onClose: () => void;
  onSaved: () => void;
  onQr: (label: string) => void;
}) {
  const [label, setLabel] = useState(profile?.profile.label ?? '');
  const [enabled, setEnabled] = useState(profile?.profile.enabled ?? true);
  // Material replacement always starts empty; summaries never contain recoverable secrets.
  const [cookies, setCookies] = useState('');
  const [refreshToken, setRefreshToken] = useState('');
  const [accessToken, setAccessToken] = useState('');
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const isSoop = platformName.toLowerCase() === 'soop';
  const isBilibili = platformName.toLowerCase() === 'bilibili';
  const isTwitch = platformName.toLowerCase() === 'twitch';
  const save = useMutation({
    mutationFn: async () => {
      const material = {
        cookies,
        refresh_token: refreshToken || null,
        access_token: accessToken || null,
        reauth_config:
          isSoop && (username || password) ? { username, password } : null,
      };
      if (profile)
        await updateCredentialProfile({
          data: {
            id: profile.profile.id,
            expected_version: profile.profile.version,
            label,
            enabled,
            ...(replace ? { replacement: material } : {}),
          },
        });
      else
        await createCredentialProfile({
          data: { owner, platform_id: platformId, label, enabled, material },
        });
    },
    onSuccess: onSaved,
  });
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
    >
      <DialogContent>
        <DialogHeader>
          <DialogTitle>
            {profile ? (
              <Trans>Edit account profile</Trans>
            ) : (
              <Trans>Add profile</Trans>
            )}
          </DialogTitle>
          <DialogDescription>
            <Trans>
              Saved immediately, independently of configuration changes.
            </Trans>
          </DialogDescription>
        </DialogHeader>
        <Label className="grid gap-2">
          <Trans>Label</Trans>
          <Input
            value={label}
            maxLength={128}
            onChange={(event) => setLabel(event.target.value)}
          />
        </Label>
        <Label className="flex items-center justify-between">
          <Trans>Enabled</Trans>
          <Switch checked={enabled} onCheckedChange={setEnabled} />
        </Label>
        {replace && (
          <>
            <p className="text-sm text-muted-foreground">
              <Trans>
                Enter a complete replacement bundle. Empty token and login
                fields clear the previous values. Stored secrets are never
                displayed.
              </Trans>
            </p>
            <Label className="grid gap-2">
              <Trans>Cookies</Trans>
              <Textarea
                autoComplete="off"
                value={cookies}
                onChange={(event) => setCookies(event.target.value)}
              />
            </Label>
            {isBilibili && (
              <Label className="grid gap-2">
                <Trans>Refresh token</Trans>
                <Input
                  type="password"
                  autoComplete="new-password"
                  value={refreshToken}
                  onChange={(event) => setRefreshToken(event.target.value)}
                />
              </Label>
            )}
            {(isBilibili || isTwitch) && (
              <Label className="grid gap-2">
                <Trans>Access token</Trans>
                <Input
                  type="password"
                  autoComplete="new-password"
                  value={accessToken}
                  onChange={(event) => setAccessToken(event.target.value)}
                />
              </Label>
            )}
            {isTwitch && (
              <p className="text-xs text-muted-foreground">
                <Trans>
                  Twitch can authenticate with an access token without cookies.
                </Trans>
              </p>
            )}
            {isSoop && (
              <>
                <Label className="grid gap-2">
                  <Trans>Username</Trans>
                  <Input
                    autoComplete="off"
                    value={username}
                    onChange={(event) => setUsername(event.target.value)}
                  />
                </Label>
                <Label className="grid gap-2">
                  <Trans>Password</Trans>
                  <Input
                    type="password"
                    autoComplete="new-password"
                    value={password}
                    onChange={(event) => setPassword(event.target.value)}
                  />
                </Label>
              </>
            )}
          </>
        )}
        {save.error && <p role="alert">{save.error.message}</p>}
        <div className="flex gap-2">
          <Button
            type="button"
            disabled={save.isPending || !label.trim()}
            onClick={() => save.mutate()}
          >
            <Trans>Save profile</Trans>
          </Button>
          {!profile && isBilibili && (
            <Button
              type="button"
              variant="outline"
              disabled={!label.trim()}
              onClick={() => onQr(label.trim())}
            >
              <Trans>QR Login</Trans>
            </Button>
          )}
        </div>
      </DialogContent>
    </Dialog>
  );
}
