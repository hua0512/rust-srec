import {
  useEffect,
  useState,
  type ComponentProps,
  type ReactNode,
} from 'react';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { ArrowDown, ArrowUp, X } from 'lucide-react';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import {
  FormControl,
  FormDescription,
  FormItem,
  FormLabel,
} from '@/components/ui/form';
import { Input } from '@/components/ui/input';
import { Switch } from '@/components/ui/switch';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { CONFIG_SELECT_CONTENT } from '@/components/config/shared/config-field';
import { cn } from '@/lib/utils';
import type {
  CredentialProfileDetail,
  CredentialSelection,
} from '@/api/schemas/credential-profiles';
import { HealthDot } from './account-health';

const MIN_ATTEMPTS = 1;
const MAX_ATTEMPTS = 10;

/** The editor's selects and inputs: compact, as befits a list's inline editor. */
const FIELD = 'h-9 rounded-lg border-border/50 bg-background/50 shadow-none';
const SELECT_TRIGGER = cn('w-full', FIELD);
const DESCRIPTION = 'text-xs text-muted-foreground';

/**
 * Free-form text while typing; a value within range is applied immediately and
 * anything else is clamped (or restored when empty) once the field loses focus.
 */
function MaxAttemptsInput({
  value,
  onChange,
  className,
  ...props
}: Omit<ComponentProps<'input'>, 'value' | 'onChange'> & {
  value: number;
  onChange: (value: number) => void;
}) {
  const [text, setText] = useState(String(value));
  useEffect(() => {
    setText((current) => (Number(current) === value ? current : String(value)));
  }, [value]);
  const parse = (raw: string) => {
    const trimmed = raw.trim();
    return /^\d+$/.test(trimmed) ? Number(trimmed) : undefined;
  };
  return (
    <Input
      {...props}
      type="number"
      inputMode="numeric"
      min={MIN_ATTEMPTS}
      max={MAX_ATTEMPTS}
      className={cn(FIELD, className)}
      value={text}
      onChange={(event) => {
        setText(event.target.value);
        const parsed = parse(event.target.value);
        if (
          parsed !== undefined &&
          parsed >= MIN_ATTEMPTS &&
          parsed <= MAX_ATTEMPTS &&
          parsed !== value
        )
          onChange(parsed);
      }}
      onBlur={() => {
        const parsed = parse(text);
        const next =
          parsed === undefined
            ? value
            : Math.max(MIN_ATTEMPTS, Math.min(MAX_ATTEMPTS, parsed));
        setText(String(next));
        if (next !== value) onChange(next);
      }}
    />
  );
}

const ROW_ICON_BUTTON = 'size-7 rounded-lg text-muted-foreground sm:size-8';

/** Marks a disabled account wherever accounts are picked. */
function DisabledMarker() {
  return (
    <Badge
      variant="secondary"
      className="h-5 shrink-0 px-1.5 text-[10px] text-muted-foreground"
    >
      <Trans>Disabled</Trans>
    </Badge>
  );
}

/**
 * An account as the pickers show it: its health, its label on one line (the
 * full label in its tooltip), the sites it is for and whether it is disabled.
 * `account` is absent for an ID the platform no longer has.
 */
function AccountName({
  account,
  unknownLabel,
  className,
}: {
  account: CredentialProfileDetail | undefined;
  unknownLabel: string;
  className?: string;
}) {
  const label = account?.profile.label ?? unknownLabel;
  return (
    <span className={cn('flex min-w-0 items-center gap-2', className)}>
      {account && <HealthDot validity={account.health?.validity} />}
      <span
        title={label}
        className={cn(
          'min-w-0 truncate',
          (!account || !account.profile.enabled) && 'text-muted-foreground',
        )}
      >
        {label}
      </span>
      {account?.sites?.length ? (
        <span className="min-w-0 truncate text-xs text-muted-foreground">
          {account.sites.join(', ')}
        </span>
      ) : null}
      {account && !account.profile.enabled && <DisabledMarker />}
    </span>
  );
}

/**
 * Edits a scope's account selection: the mode, then the fixed account or the
 * pool's ordered members, its failover and its attempt budget.
 */
export function CredentialSelectionEditor({
  value,
  onChange,
  accounts,
  singleAccount = false,
  emptyHint,
  hint,
}: {
  value: CredentialSelection | undefined;
  onChange: (value: CredentialSelection) => void;
  /** The platform's accounts with their health. */
  accounts: CredentialProfileDetail[];
  /**
   * Offer only inherit, none and one fixed account; inheriting uses the
   * account set up for the streamer's site, if any.
   */
  singleAccount?: boolean;
  /** Shown while the platform has no accounts to choose from. */
  emptyHint?: ReactNode;
  /** Appended to the mode's description. */
  hint?: ReactNode;
}) {
  const { i18n } = useLingui();
  const unknownAccount = i18n._(msg`Unknown account`);
  const profiles = accounts.map((account) => account.profile);
  const accountOf = (id: string) =>
    accounts.find((account) => account.profile.id === id);
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
          : profiles.slice(0, 1).map(({ id }) => id),
        strategy: mode === 'round_robin' ? 'round_robin' : 'priority',
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
  const addable = accounts.filter(
    (account) => !ids.includes(account.profile.id),
  );
  return (
    <div className="max-w-2xl space-y-4">
      <FormItem className="gap-1.5">
        <FormLabel>
          <Trans>Account selection</Trans>
        </FormLabel>
        <Select
          // Nothing saved means the scope inherits.
          value={
            value?.mode === 'pool' ? value.strategy : (value?.mode ?? 'inherit')
          }
          onValueChange={changeMode}
        >
          <FormControl>
            <SelectTrigger className={SELECT_TRIGGER}>
              <SelectValue />
            </SelectTrigger>
          </FormControl>
          <SelectContent className={CONFIG_SELECT_CONTENT}>
            <SelectItem value="inherit">
              {singleAccount ? (
                <Trans>Inherit (the site&apos;s account)</Trans>
              ) : (
                <Trans>Inherit</Trans>
              )}
            </SelectItem>
            <SelectItem value="none">
              <Trans>No authentication</Trans>
            </SelectItem>
            <SelectItem value="fixed" disabled={!profiles.length}>
              <Trans>Fixed account</Trans>
            </SelectItem>
            {!singleAccount && (
              <>
                <SelectItem value="priority" disabled={!profiles.length}>
                  <Trans>Primary and backups</Trans>
                </SelectItem>
                <SelectItem value="round_robin" disabled={!profiles.length}>
                  <Trans>Round-robin pool</Trans>
                </SelectItem>
              </>
            )}
          </SelectContent>
        </Select>
        {(value?.mode === 'fixed' || value?.mode === 'pool' || hint) && (
          <FormDescription className={DESCRIPTION}>
            {(value?.mode === 'fixed' || value?.mode === 'pool') && (
              <Trans>
                A recording keeps using the account it started with.
              </Trans>
            )}
            {hint && <> {hint}</>}
          </FormDescription>
        )}
      </FormItem>
      {!profiles.length && emptyHint && (
        <p className={DESCRIPTION}>{emptyHint}</p>
      )}
      {value?.mode === 'fixed' && (
        <FormItem className="gap-1.5">
          <FormLabel>
            <Trans>Account</Trans>
          </FormLabel>
          <Select
            value={value.credential_id}
            onValueChange={(credential_id) =>
              onChange({ mode: 'fixed', credential_id })
            }
          >
            <FormControl>
              <SelectTrigger className={SELECT_TRIGGER}>
                <SelectValue />
              </SelectTrigger>
            </FormControl>
            <SelectContent className={CONFIG_SELECT_CONTENT}>
              {!accountOf(value.credential_id) && value.credential_id && (
                <SelectItem value={value.credential_id}>
                  {unknownAccount}
                </SelectItem>
              )}
              {accounts.map((account) => (
                <SelectItem key={account.profile.id} value={account.profile.id}>
                  <AccountName
                    account={account}
                    unknownLabel={unknownAccount}
                  />
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </FormItem>
      )}
      {value?.mode === 'pool' && (
        <>
          <div className="space-y-1.5">
            <p className="text-sm font-medium">
              {value.strategy === 'priority' ? (
                <Trans>Accounts, in the order they are tried</Trans>
              ) : (
                <Trans>Accounts</Trans>
              )}
            </p>
            <ol className="divide-y divide-border/50 rounded-lg border border-border/50">
              {ids.map((id, index) => (
                <li
                  key={id}
                  className="flex min-h-10 items-center gap-2 py-1 pr-1 pl-3"
                >
                  <span className="w-4 shrink-0 text-xs text-muted-foreground tabular-nums">
                    {index + 1}
                  </span>
                  <AccountName
                    account={accountOf(id)}
                    unknownLabel={unknownAccount}
                    className="flex-1 text-sm"
                  />
                  <div className="flex shrink-0 items-center">
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon"
                      className={ROW_ICON_BUTTON}
                      disabled={index === 0}
                      onClick={() => move(index, -1)}
                      aria-label={i18n._(msg`Move account up`)}
                    >
                      <ArrowUp />
                    </Button>
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon"
                      className={ROW_ICON_BUTTON}
                      disabled={index === ids.length - 1}
                      onClick={() => move(index, 1)}
                      aria-label={i18n._(msg`Move account down`)}
                    >
                      <ArrowDown />
                    </Button>
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon"
                      className={cn(
                        ROW_ICON_BUTTON,
                        'hover:bg-destructive/10 hover:text-destructive',
                      )}
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
                  </div>
                </li>
              ))}
            </ol>
            <Select
              value=""
              disabled={!addable.length}
              onValueChange={(id) =>
                onChange({ ...value, credential_ids: [...ids, id] })
              }
            >
              <SelectTrigger className={cn(SELECT_TRIGGER, 'border-dashed')}>
                <SelectValue placeholder={i18n._(msg`Add account`)} />
              </SelectTrigger>
              <SelectContent className={CONFIG_SELECT_CONTENT}>
                {addable.map((account) => (
                  <SelectItem
                    key={account.profile.id}
                    value={account.profile.id}
                  >
                    <AccountName
                      account={account}
                      unknownLabel={unknownAccount}
                    />
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          <div className="flex flex-wrap items-center gap-x-6 gap-y-3">
            <FormItem className="flex min-w-0 flex-1 basis-64 items-center gap-2.5">
              <FormControl>
                <Switch
                  checked={value.failover}
                  onCheckedChange={(failover) =>
                    onChange({ ...value, failover })
                  }
                />
              </FormControl>
              <FormLabel className="font-normal leading-snug">
                <Trans>Switch to the next account when one stops working</Trans>
              </FormLabel>
            </FormItem>
            <FormItem className="flex shrink-0 items-center gap-2.5">
              <FormLabel className="font-normal">
                <Trans>Maximum attempts (1–10)</Trans>
              </FormLabel>
              <FormControl>
                <MaxAttemptsInput
                  className="h-8 w-16"
                  value={value.max_attempts}
                  onChange={(max_attempts) =>
                    onChange({ ...value, max_attempts })
                  }
                />
              </FormControl>
            </FormItem>
          </div>
        </>
      )}
    </div>
  );
}
