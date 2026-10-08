import { Link } from '@tanstack/react-router';
import { Trans } from '@lingui/react/macro';
import { msg, plural, t } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import type { MessageDescriptor } from '@lingui/core';
import { Radio } from 'lucide-react';
import { formatRelativeTime, formatShortRelativeTime } from '@/lib/date-utils';
import { formatPlatformName } from '@/lib/format';
import { cn } from '@/lib/utils';
import type {
  CredentialHealthReason,
  CredentialProfileDetail,
  ProfileReferences,
  RecordingReference,
  SelectionReference,
} from '@/api/schemas/credential-profiles';
import {
  DotSeparated,
  NAME_LINK,
  NameList,
} from '@/components/shared/list-panel';
import { NAME_ICON, OwnerLink, TERM } from '@/components/shared/owner-link';
import { ProxyRouteValue } from '@/components/proxies/proxy-route-label';
import {
  VALIDITY_STYLES,
  attentionReason,
  validityStyle,
} from './account-health';

/** Names shown before the rest collapse into "+N more". */
const SHOWN_NAMES = 3;

function SelectionName({ reference }: { reference: SelectionReference }) {
  const { i18n } = useLingui();
  const { owner, name } = reference;
  switch (owner.type) {
    case 'platform': {
      const label = formatPlatformName(name);
      return (
        <OwnerLink
          owner={{ type: 'platform', id: owner.platform_id }}
          title={i18n._(msg`Platform ${label}`)}
        >
          {label}
        </OwnerLink>
      );
    }
    case 'template':
      return (
        <OwnerLink
          owner={{ type: 'template', id: owner.template_id }}
          title={i18n._(msg`Template ${name}`)}
        >
          {name}
        </OwnerLink>
      );
    case 'streamer':
      return (
        <OwnerLink
          owner={{ type: 'streamer', id: owner.streamer_id }}
          title={i18n._(msg`Streamer ${name}`)}
        >
          {name}
        </OwnerLink>
      );
  }
}

function RecordingName({ reference }: { reference: RecordingReference }) {
  const name = reference.streamer_name;
  const content = (
    <>
      <Radio className={cn(NAME_ICON, 'text-red-500')} />
      {name}
    </>
  );
  return reference.streamer_id ? (
    <Link
      to="/streamers/$id/edit"
      params={{ id: reference.streamer_id }}
      className={NAME_LINK}
    >
      {content}
    </Link>
  ) : (
    <span className="font-medium text-foreground/80">{content}</span>
  );
}

/** Which configurations select the account and which recordings use it. */
export function AccountReferences({
  references,
  limit = SHOWN_NAMES,
  className,
}: {
  references: ProfileReferences;
  limit?: number;
  className?: string;
}) {
  const { selections, recordings } = references;
  if (selections.length === 0 && recordings.length === 0) return null;
  return (
    <div className={className}>
      {selections.length > 0 && (
        <p>
          <Trans>Selected by</Trans>{' '}
          <NameList
            items={selections}
            limit={limit}
            label={selectionLabel}
            render={(reference) => <SelectionName reference={reference} />}
          />
        </p>
      )}
      {recordings.length > 0 && (
        <p>
          <Trans context="live recordings using an account">Recording</Trans>{' '}
          <NameList
            items={recordings}
            limit={limit}
            label={(reference) => reference.streamer_name}
            render={(reference) => <RecordingName reference={reference} />}
          />
        </p>
      )}
    </div>
  );
}

/** Why the account's health is what it is; a manual validation needs no note. */
export const REASON_LABELS: Record<
  Exclude<CredentialHealthReason, 'manual_validation'>,
  MessageDescriptor
> = {
  login_required: msg`The platform asked for a new login`,
  refresh_failed: msg`The last refresh failed`,
  repair_required: msg`A check found it needs a refresh`,
  authentication_failed: msg`Rejected during use; a refresh is queued`,
};

/** Configuration names the summary line spells out before "+N". */
const SUMMARY_NAMES = 2;

interface SummarySegment {
  text: string;
  className?: string;
}

/**
 * The summary under an account's name. An account that needs the user
 * says why, in the colour of its state; any other says who selects it and when
 * it was last used. Both end with what is known of its timing.
 */
export function AccountSummaryLine({
  detail,
  className,
}: {
  detail: CredentialProfileDetail;
  className?: string;
}) {
  const { i18n } = useLingui();
  const ago = (time: number) => formatShortRelativeTime(time, i18n.locale);
  const { profile, health, references } = detail;
  const attention = profile.enabled ? attentionReason(health) : undefined;
  const segments: SummarySegment[] = [];
  const lastCheck = health?.last_check_at;
  const checked = () => {
    if (lastCheck == null) return;
    const when = ago(lastCheck);
    segments.push({ text: t(i18n)`checked ${when}` });
  };
  if (!profile.enabled) {
    segments.push({ text: t(i18n)`Not used while disabled` });
    checked();
  } else if (attention) {
    const failures = health?.refresh_failure_count ?? 0;
    const reason = health?.reason_code;
    segments.push({
      text:
        attention === 'refresh_failing'
          ? t(i18n)`${plural(failures, {
              one: 'Refresh failed # time',
              other: 'Refresh failed # times',
            })}`
          : i18n._(
              reason && reason !== 'manual_validation'
                ? REASON_LABELS[reason]
                : VALIDITY_STYLES.invalid.label,
            ),
      className: validityStyle(health?.validity).text,
    });
    checked();
  } else {
    const names = references.selections.map(selectionLabel);
    const shown = names.slice(0, SUMMARY_NAMES).join(', ');
    const rest = names.length - SUMMARY_NAMES;
    segments.push({
      text: !names.length
        ? t(i18n)`Not selected anywhere`
        : rest > 0
          ? `${shown} +${rest}`
          : shown,
    });
    const recordings = references.recordings.length;
    if (recordings > 0)
      segments.push({
        text: t(i18n)`${plural(recordings, {
          one: '# live recording',
          other: '# live recordings',
        })}`,
      });
    const lastUsed = profile.last_used_at;
    if (lastUsed != null) {
      const when = ago(lastUsed);
      segments.push({ text: t(i18n)`used ${when}` });
    } else checked();
  }
  const renewal = detail.next_renewal_at;
  if (profile.enabled && renewal != null) {
    if (renewal <= Date.now()) segments.push({ text: t(i18n)`renewal due` });
    else {
      const when = ago(renewal);
      segments.push({ text: t(i18n)`renews ${when}` });
    }
  }
  return (
    <p
      className={className}
      title={segments.map((segment) => segment.text).join(' · ')}
    >
      <DotSeparated
        parts={segments.map(({ text, className }) => ({
          content: text,
          className,
        }))}
      />
    </p>
  );
}

function selectionLabel(reference: SelectionReference) {
  return reference.owner.type === 'platform'
    ? formatPlatformName(reference.name)
    : reference.name;
}

/** A time in the past or future, relative, with the exact time on hover. */
function When({ time }: { time: number }) {
  const { i18n } = useLingui();
  return (
    <time
      dateTime={new Date(time).toISOString()}
      title={new Date(time).toLocaleString(i18n.locale)}
    >
      {formatRelativeTime(time, i18n.locale)}
    </time>
  );
}

const MUTED_VALUE = 'text-muted-foreground/80';

/**
 * Everything known about an account, as a short definition list: every
 * configuration selecting it and recording with it, when it was used, checked,
 * refreshed and is next renewed, what went wrong, and how it connects.
 */
export function AccountDetails({
  detail,
  className,
}: {
  detail: CredentialProfileDetail;
  className?: string;
}) {
  const { i18n } = useLingui();
  const { profile, health, references, capabilities } = detail;
  const { selections, recordings } = references;
  const lastUsed = profile.last_used_at;
  const lastCheck = health?.last_check_at;
  const lastRefresh = health?.last_refresh_at;
  const renewal = detail.next_renewal_at;
  const failures = health?.refresh_failure_count ?? 0;
  const reason = health?.reason_code;
  // Warnings take the colour of the account's state: red once it needs a new
  // login, amber while a refresh may still fix it.
  const warningTone =
    health?.validity === 'invalid'
      ? VALIDITY_STYLES.invalid.text
      : VALIDITY_STYLES.needs_refresh.text;
  const warnings: string[] = [];
  if (failures > 0)
    warnings.push(
      t(i18n)`${plural(failures, {
        one: '# failed refresh',
        other: '# failed refreshes in a row',
      })}`,
    );
  if (reason && reason !== 'manual_validation')
    warnings.push(i18n._(REASON_LABELS[reason]));
  return (
    <div className={cn('space-y-2 text-xs', className)}>
      <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-1.5">
        <dt className={TERM}>
          <Trans>Selected by</Trans>
        </dt>
        <dd>
          {selections.length ? (
            <NameList
              items={selections}
              limit={selections.length}
              label={selectionLabel}
              render={(reference) => <SelectionName reference={reference} />}
            />
          ) : (
            <span className={MUTED_VALUE}>
              <Trans>Not selected anywhere</Trans>
            </span>
          )}
        </dd>
        {recordings.length > 0 && (
          <>
            <dt className={TERM}>
              <Trans context="live recordings using an account">
                Recording
              </Trans>
            </dt>
            <dd>
              <NameList
                items={recordings}
                limit={recordings.length}
                label={(reference) => reference.streamer_name}
                render={(reference) => <RecordingName reference={reference} />}
              />
            </dd>
          </>
        )}
        <dt className={TERM}>
          <Trans>Last used</Trans>
        </dt>
        <dd>
          {lastUsed != null ? (
            <When time={lastUsed} />
          ) : (
            <span className={MUTED_VALUE}>
              <Trans context="an account never used or checked">Never</Trans>
            </span>
          )}
        </dd>
        <dt className={TERM}>
          <Trans>Last checked</Trans>
        </dt>
        <dd>
          {lastCheck != null ? (
            <When time={lastCheck} />
          ) : (
            <span className={MUTED_VALUE}>
              {capabilities.validate ? (
                <Trans context="an account never used or checked">Never</Trans>
              ) : (
                <Trans>Validation not supported</Trans>
              )}
            </span>
          )}
        </dd>
        {lastRefresh != null && (
          <>
            <dt className={TERM}>
              <Trans>Last refreshed</Trans>
            </dt>
            <dd>
              <When time={lastRefresh} />
            </dd>
          </>
        )}
        {renewal != null && (
          <>
            <dt className={TERM}>
              <Trans>Next renewal</Trans>
            </dt>
            <dd>
              {renewal <= Date.now() ? (
                <Trans>Due at its next use</Trans>
              ) : (
                <When time={renewal} />
              )}
            </dd>
          </>
        )}
        {warnings.length > 0 && (
          <>
            <dt className={TERM}>
              <Trans>Problems</Trans>
            </dt>
            <dd className={warningTone}>{warnings.join(' · ')}</dd>
          </>
        )}
        <dt className={TERM}>
          <Trans>Proxy</Trans>
        </dt>
        <dd>
          <ProxyRouteValue route={profile.proxy_route} account />
        </dd>
      </dl>
      {!profile.enabled && (
        <p className={MUTED_VALUE}>
          <Trans>
            Disabled accounts are not used, checked or refreshed. You can still
            edit them or log in again.
          </Trans>
        </p>
      )}
    </div>
  );
}
