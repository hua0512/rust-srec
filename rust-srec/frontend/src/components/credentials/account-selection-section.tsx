import { Fragment, useId, useState, type ReactNode } from 'react';
import { useQuery } from '@tanstack/react-query';
import { Trans } from '@lingui/react/macro';
import { msg, plural, t } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { AlertTriangle, ChevronDown, Info, Pin } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Disclosure, DisclosureContent } from '@/components/ui/disclosure';
import { cn } from '@/lib/utils';
import {
  effectiveSelectionQueryOptions,
  platformAccountsQueryOptions,
  platformCapabilitiesQueryOptions,
} from '@/api/credential-profiles';
import type {
  CredentialOwner,
  CredentialProfileDetail,
  CredentialSelection,
  EffectiveCredentialSelection,
  UnavailableReason,
} from '@/api/schemas/credential-profiles';
import { HealthDot } from './account-health';
import { DotSeparated, PANEL_BAND } from '@/components/shared/list-panel';
import { Callout } from '@/components/shared/callout';
import { CredentialSelectionEditor } from './credential-selection-editor';

/**
 * Why the saved selection cannot sign in. `single` is set when the selection
 * names one account, so the advice speaks of that account alone.
 */
function UnavailableNotice({
  reason,
  single,
}: {
  reason: UnavailableReason;
  single: boolean;
}) {
  switch (reason) {
    case 'login_required':
      return single ? (
        <Trans>
          This account needs a new login. Log in again or replace its cookies.
        </Trans>
      ) : (
        <Trans>
          All selected accounts need a new login. Log in again to one of them or
          replace its cookies.
        </Trans>
      );
    case 'profiles_disabled':
      return single ? (
        <Trans>
          This account is disabled. Enable it or choose another account.
        </Trans>
      ) : (
        <Trans>
          All selected accounts are disabled. Enable one or change the
          selection.
        </Trans>
      );
    case 'bound_profile_unavailable':
      return (
        <Trans>
          The account the active recording uses can no longer be used.
        </Trans>
      );
    case 'binding_policy_changed':
      return (
        <Trans>
          The selection changed during the active recording. The recording
          switches to the new selection when it next reconnects.
        </Trans>
      );
    case 'attempts_exhausted':
      return single ? (
        <Trans>
          Signing in with this account kept failing. Check its status or log in
          again.
        </Trans>
      ) : (
        <Trans>
          None of the selected accounts could sign in within the attempt limit.
          Check their status or log in again to one of them.
        </Trans>
      );
    case 'unknown':
      return (
        <Trans>
          Signing in is temporarily unavailable. Check the account status and
          try again.
        </Trans>
      );
    default:
      return reason satisfies never;
  }
}

/** How many accounts a selection names; inherit and none name none. */
function selectedCount(selection: CredentialSelection | undefined | null) {
  if (selection?.mode === 'fixed') return 1;
  if (selection?.mode === 'pool') return selection.credential_ids.length;
  return 0;
}

/**
 * An account named in the summary, with its health; muted when disabled.
 * `before` and `after` (an arrow, a comma) wrap with the name, never alone.
 */
function SummaryName({
  account,
  unknownLabel,
  before,
  after,
}: {
  account: CredentialProfileDetail | undefined;
  unknownLabel: string;
  before?: ReactNode;
  after?: ReactNode;
}) {
  const { i18n } = useLingui();
  const disabled = account && !account.profile.enabled;
  return (
    <span className="inline-flex max-w-full items-center gap-1.5 align-bottom">
      {before}
      {account && <HealthDot validity={account.health?.validity} />}
      <span
        className={cn(
          'truncate font-medium',
          (!account || disabled) && 'text-muted-foreground',
        )}
      >
        {account?.profile.label ?? unknownLabel}
        {disabled && (
          <span className="sr-only"> ({i18n._(msg`Disabled`)})</span>
        )}
      </span>
      {after}
    </span>
  );
}

/** The accounts of a selection in order, joined by arrows or commas. */
function SummaryNames({
  ids,
  accounts,
  separator,
}: {
  ids: string[];
  accounts: CredentialProfileDetail[];
  separator: 'then' | 'and';
}) {
  const { i18n } = useLingui();
  const unknownLabel = i18n._(msg`Unknown account`);
  return (
    <>
      {ids.map((id, index) => (
        <Fragment key={id}>
          {index > 0 && ' '}
          <SummaryName
            account={accounts.find((account) => account.profile.id === id)}
            unknownLabel={unknownLabel}
            before={
              separator === 'then' &&
              index > 0 && (
                <span className="text-muted-foreground">
                  <span aria-hidden="true">→</span>
                  <span className="sr-only"> {i18n._(msg`then`)} </span>
                </span>
              )
            }
            after={
              separator === 'and' &&
              index < ids.length - 1 && <span className="-ml-1.5">,</span>
            }
          />
        </Fragment>
      ))}
    </>
  );
}

/** What a selection records with, as one sentence. */
function SelectionSentence({
  selection,
  accounts,
}: {
  selection: CredentialSelection;
  accounts: CredentialProfileDetail[];
}) {
  switch (selection.mode) {
    case 'inherit':
    case 'none':
      return <Trans>No account — records signed out</Trans>;
    case 'fixed': {
      const names = (
        <SummaryNames
          ids={[selection.credential_id]}
          accounts={accounts}
          separator="then"
        />
      );
      return (
        <Trans>
          <span className="text-muted-foreground">Recording uses</span> {names}
        </Trans>
      );
    }
    default: {
      if (selection.strategy === 'round_robin') {
        const names = (
          <SummaryNames
            ids={selection.credential_ids}
            accounts={accounts}
            separator="and"
          />
        );
        return (
          <Trans>
            <span className="text-muted-foreground">
              Recording rotates between
            </span>{' '}
            {names}
          </Trans>
        );
      }
      const names = (
        <SummaryNames
          ids={selection.credential_ids}
          accounts={accounts}
          separator="then"
        />
      );
      return (
        <Trans>
          <span className="text-muted-foreground">Recording uses</span> {names}
        </Trans>
      );
    }
  }
}

/** How a pool behaves when an account fails; nothing for other modes. */
function poolBehaviour(
  selection: CredentialSelection,
  i18n: ReturnType<typeof useLingui>['i18n'],
): string[] {
  if (selection.mode !== 'pool') return [];
  const attempts = selection.max_attempts;
  return [
    selection.failover ? t(i18n)`Failover on` : t(i18n)`Failover off`,
    t(i18n)`${plural(attempts, {
      one: 'up to # try',
      other: 'up to # tries',
    })}`,
  ];
}

/** Where an inherited selection comes from. */
function inheritedFrom(
  owner: CredentialOwner,
  i18n: ReturnType<typeof useLingui>['i18n'],
): string {
  switch (owner.type) {
    case 'platform':
      return t(i18n)`Uses the platform's setting`;
    case 'template':
      return t(i18n)`Uses the template's setting`;
    default:
      return t(i18n)`Uses the streamer's setting`;
  }
}

/**
 * The selection in words: a sentence naming the accounts recording uses, in
 * order and with their health, and a muted line with where an inherited
 * selection comes from and how a pool fails over. The effective selection
 * describes the saved configuration, so an inherit selection is resolved only
 * while the form still matches it.
 */
function SelectionSummary({
  selection,
  accounts,
  effective,
  singleAccount = false,
  topLevel = false,
  dirty = false,
  action,
}: {
  selection: CredentialSelection | undefined;
  accounts: CredentialProfileDetail[];
  effective: EffectiveCredentialSelection | undefined;
  /** Inheriting means no account, as for Streamlink streamers. */
  singleAccount?: boolean;
  /** The scope has nothing to inherit from, as for a platform. */
  topLevel?: boolean;
  /** The form holds a selection that is not saved yet. */
  dirty?: boolean;
  /** Beside the sentence, such as the button that opens the editor. */
  action?: ReactNode;
}) {
  const { i18n } = useLingui();
  let sentence: ReactNode;
  let details: string[];
  if (selection && selection.mode !== 'inherit') {
    sentence = <SelectionSentence selection={selection} accounts={accounts} />;
    details = poolBehaviour(selection, i18n);
  } else if (singleAccount) {
    sentence = <Trans>No account — records signed out</Trans>;
    details = [];
  } else if (
    effective &&
    (!effective.configured || effective.configured.mode === 'inherit')
  ) {
    const resolved = effective.resolved;
    if (!resolved) {
      sentence = <Trans>No account — records signed out</Trans>;
      details = topLevel
        ? []
        : [t(i18n)`Nothing it inherits from chooses an account`];
    } else {
      // The inherited accounts come with the effective selection, so their
      // health shows even before the platform's list has loaded.
      sentence = (
        <SelectionSentence
          selection={resolved.selection}
          accounts={[...effective.candidates, ...accounts]}
        />
      );
      details = [
        inheritedFrom(resolved.owner, i18n),
        ...poolBehaviour(resolved.selection, i18n),
      ];
    }
  } else {
    sentence = <Trans>Uses the inherited setting</Trans>;
    details = effective
      ? [t(i18n)`Save this configuration to see what it inherits.`]
      : [];
  }
  // Narrow panels keep the action beside the sentence and give the muted
  // line the full width under both.
  return (
    <div className="grid grid-cols-[minmax(0,1fr)_auto] items-start gap-x-3 gap-y-0.5 @md:items-center">
      <p className="text-sm leading-6">{sentence}</p>
      {action && <div className="@md:row-span-2">{action}</div>}
      {(dirty || details.length > 0) && (
        <p className="col-span-2 text-xs text-muted-foreground @md:col-span-1">
          <DotSeparated
            parts={[
              ...(dirty
                ? [
                    {
                      content: (
                        <>
                          <span className="mr-1.5 inline-block size-1.5 rounded-full bg-current align-middle" />
                          <Trans>Unsaved</Trans>
                        </>
                      ),
                      className: 'text-amber-600 dark:text-amber-400',
                    },
                  ]
                : []),
              ...details.map((detail) => ({ content: detail })),
            ]}
          />
        </p>
      )}
    </div>
  );
}

/**
 * Chooses which of the platform's accounts `scope` uses, saved together with
 * the configuration form: a summary of the selection with a Change button that
 * expands the editor. Notices about the saved selection (an unavailable one,
 * the account pinned to an active recording) stay under the summary while the
 * editor is closed. Streamlink accounts are chosen per streamer, so the
 * platform and templates only explain that.
 */
export function AccountSelectionSection({
  scope,
  platformId,
  selection,
  onSelectionChange,
  dirty = false,
}: {
  scope: CredentialOwner;
  platformId: string;
  selection?: CredentialSelection;
  onSelectionChange: (selection: CredentialSelection) => void;
  /** The form holds a selection that is not saved yet. */
  dirty?: boolean;
}) {
  const { i18n } = useLingui();
  const [editing, setEditing] = useState(false);
  const editorId = useId();
  const accounts = useQuery(platformAccountsQueryOptions(platformId));
  const effective = useQuery(effectiveSelectionQueryOptions(scope, platformId));
  // The platform page lists its accounts with an add button; other scopes
  // link to it from the panel's header.
  const onPlatformPage = scope.type === 'platform';
  const capabilities = useQuery(platformCapabilitiesQueryOptions(platformId));
  const perStreamer = capabilities.data?.per_streamer_selection;
  const labelOf = (id: string) =>
    accounts.data?.find((entry) => entry.profile.id === id)?.profile.label ??
    i18n._(msg`Unknown account`);
  // Recovering from an unavailable selection happens on the platform page,
  // which the panel's header links to, so the notice does not repeat the link.
  // The reason describes the saved selection, or the one it inherits.
  const unavailableNotice = effective.data?.unavailable_reason && (
    <Callout tone="warning" icon={AlertTriangle} role="status">
      <UnavailableNotice
        reason={effective.data.unavailable_reason}
        single={
          selectedCount(
            effective.data.resolved?.selection ?? effective.data.configured,
          ) === 1
        }
      />
    </Callout>
  );
  const pinnedNotice = effective.data?.active_binding?.identity.profile_id && (
    <Callout tone="info" icon={Pin}>
      <Trans>In use by the active recording</Trans>:{' '}
      <span className="font-medium">
        {labelOf(effective.data.active_binding.identity.profile_id)}
      </span>
    </Callout>
  );
  const notices = (pinnedNotice || unavailableNotice) && (
    <div className="space-y-2 px-3 pb-3 sm:px-4">
      {pinnedNotice}
      {unavailableNotice}
    </div>
  );

  if (perStreamer && scope.type !== 'streamer')
    return (
      <div className={cn(PANEL_BAND, 'pt-3')}>
        <div className="px-3 pb-3 sm:px-4">
          <Callout tone="info" icon={Info}>
            {onPlatformPage ? (
              <Trans>
                Streamlink serves many different sites, so its accounts are
                chosen on each streamer. Add accounts here, then pick one in a
                streamer&apos;s settings.
              </Trans>
            ) : (
              <Trans>
                Streamlink serves many different sites, so its accounts are
                chosen on each streamer, not in a template.
              </Trans>
            )}
          </Callout>
        </div>
        {notices}
      </div>
    );
  return (
    <Disclosure open={editing} onOpenChange={setEditing} className={PANEL_BAND}>
      <div className="px-3 py-3 sm:px-4">
        <SelectionSummary
          selection={selection}
          accounts={accounts.data ?? []}
          effective={effective.data}
          singleAccount={perStreamer}
          topLevel={onPlatformPage}
          dirty={dirty}
          action={
            // The modes offered depend on the platform's selection rules.
            capabilities.data && (
              <Button
                type="button"
                variant="outline"
                size="sm"
                aria-expanded={editing}
                aria-controls={editorId}
                data-state={editing ? 'open' : 'closed'}
                className="rs-disclosure-trigger h-8 shrink-0 gap-1 rounded-lg"
                onClick={() => setEditing(!editing)}
              >
                <Trans>Change</Trans>
                <span className="rs-disclosure-chevron">
                  <ChevronDown className="size-3.5" />
                </span>
              </Button>
            )
          }
        />
      </div>
      {notices}
      <DisclosureContent id={editorId} className="px-3 pt-1 pb-4 sm:px-4">
        <CredentialSelectionEditor
          value={selection}
          onChange={onSelectionChange}
          accounts={accounts.data ?? []}
          singleAccount={perStreamer ?? false}
          hint={
            perStreamer && (
              <Trans>
                Streamlink receives only the account&apos;s cookies, and the
                account is not checked automatically.
              </Trans>
            )
          }
          emptyHint={
            accounts.data &&
            (onPlatformPage ? (
              <Trans>
                This platform has no accounts yet. Use Add account above to
                create one.
              </Trans>
            ) : (
              <Trans>
                This platform has no accounts yet. Add them in the platform
                settings.
              </Trans>
            ))
          }
        />
        <p className="mt-4 text-xs text-muted-foreground">
          <Trans>Saved together with this configuration.</Trans>
        </p>
      </DisclosureContent>
    </Disclosure>
  );
}
