import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { ArrowDown, ArrowUp, X } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { Switch } from '@/components/ui/switch';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import type {
  CredentialProfileSummary,
  CredentialSelection,
} from '@/api/schemas/credential-profiles';

export function CredentialSelectionEditor({
  value,
  onChange,
  profiles,
}: {
  value: CredentialSelection | undefined;
  onChange: (value: CredentialSelection) => void;
  profiles: CredentialProfileSummary[];
}) {
  const { i18n } = useLingui();
  const ids =
    value?.mode === 'pool'
      ? value.credential_ids
      : value?.mode === 'fixed'
        ? [value.credential_id]
        : [];
  const changeMode = (mode: string) => {
    if (mode === 'inherit' || mode === 'none') onChange({ mode });
    else if (mode === 'fixed')
      onChange({ mode, credential_id: ids[0] ?? profiles[0]?.id ?? '' });
    else
      onChange({
        mode: 'pool',
        credential_ids: ids.length
          ? ids
          : profiles.slice(0, 1).map((profile) => profile.id),
        strategy: mode === 'priority' ? 'priority' : 'round_robin',
        failover: true,
        max_attempts: 3,
      });
  };
  const move = (index: number, delta: number) => {
    if (value?.mode !== 'pool') return;
    const reordered = [...value.credential_ids];
    [reordered[index], reordered[index + delta]] = [
      reordered[index + delta],
      reordered[index],
    ];
    onChange({ ...value, credential_ids: reordered });
  };
  return (
    <div className="space-y-4">
      <Label>
        <Trans>Account selection</Trans>
      </Label>
      <Select
        value={
          value?.mode === 'pool' ? value.strategy : (value?.mode ?? 'legacy')
        }
        onValueChange={changeMode}
      >
        <SelectTrigger>
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          {!value && (
            <SelectItem value="legacy" disabled>
              <Trans>Legacy credentials</Trans>
            </SelectItem>
          )}
          <SelectItem value="inherit">
            <Trans>Inherit</Trans>
          </SelectItem>
          <SelectItem value="none">
            <Trans>No authentication</Trans>
          </SelectItem>
          <SelectItem value="fixed" disabled={!profiles.length}>
            <Trans>Fixed account</Trans>
          </SelectItem>
          <SelectItem value="round_robin" disabled={!profiles.length}>
            <Trans>Round-robin pool</Trans>
          </SelectItem>
          <SelectItem value="priority" disabled={!profiles.length}>
            <Trans>Primary and backups</Trans>
          </SelectItem>
        </SelectContent>
      </Select>
      {value?.mode === 'fixed' && (
        <Select
          value={value.credential_id}
          onValueChange={(credential_id) =>
            onChange({ mode: 'fixed', credential_id })
          }
        >
          <SelectTrigger>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {profiles.map((profile) => (
              <SelectItem key={profile.id} value={profile.id}>
                {profile.label} ({profile.owner.type})
                {!profile.enabled && <Trans> — disabled</Trans>}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      )}
      {value?.mode === 'pool' && (
        <>
          <ol className="space-y-2">
            {ids.map((id, index) => (
              <li key={id} className="flex items-center gap-2">
                <span className="flex-1">
                  {index + 1}.{' '}
                  {profiles.find((profile) => profile.id === id)?.label ?? id}
                </span>
                <Button
                  type="button"
                  variant="outline"
                  size="icon"
                  disabled={index === 0}
                  onClick={() => move(index, -1)}
                  aria-label={i18n._(msg`Move account up`)}
                >
                  <ArrowUp />
                </Button>
                <Button
                  type="button"
                  variant="outline"
                  size="icon"
                  disabled={index === ids.length - 1}
                  onClick={() => move(index, 1)}
                  aria-label={i18n._(msg`Move account down`)}
                >
                  <ArrowDown />
                </Button>
                <Button
                  type="button"
                  variant="outline"
                  size="icon"
                  disabled={ids.length === 1}
                  onClick={() =>
                    onChange({
                      ...value,
                      credential_ids: ids.filter((entry) => entry !== id),
                    })
                  }
                  aria-label={i18n._(msg`Remove account`)}
                >
                  <X />
                </Button>
              </li>
            ))}
          </ol>
          <Select
            value=""
            onValueChange={(id) =>
              onChange({ ...value, credential_ids: [...ids, id] })
            }
          >
            <SelectTrigger>
              <SelectValue placeholder={i18n._(msg`Add account`)} />
            </SelectTrigger>
            <SelectContent>
              {profiles
                .filter((profile) => !ids.includes(profile.id))
                .map((profile) => (
                  <SelectItem key={profile.id} value={profile.id}>
                    {profile.label} ({profile.owner.type})
                  </SelectItem>
                ))}
            </SelectContent>
          </Select>
          <Label className="flex items-center justify-between">
            <Trans>Fail over on confirmed account failure</Trans>
            <Switch
              checked={value.failover}
              onCheckedChange={(failover) => onChange({ ...value, failover })}
            />
          </Label>
          <Label className="grid gap-2">
            <Trans>Maximum attempts (1–10)</Trans>
            <Input
              type="number"
              min={1}
              max={10}
              value={value.max_attempts}
              onChange={(event) =>
                onChange({
                  ...value,
                  max_attempts: Math.max(
                    1,
                    Math.min(10, Number(event.target.value) || 1),
                  ),
                })
              }
            />
          </Label>
        </>
      )}
      <p className="text-xs text-muted-foreground">
        <Trans>
          Selection changes take effect when you save this configuration. A
          recording keeps its selected account.
        </Trans>
      </p>
    </div>
  );
}
