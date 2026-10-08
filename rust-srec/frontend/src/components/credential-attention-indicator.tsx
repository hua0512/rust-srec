import { useState } from 'react';
import { Link } from '@tanstack/react-router';
import { useQuery } from '@tanstack/react-query';
import { KeyRound } from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { msg, plural, t } from '@lingui/core/macro';
import type { I18n } from '@lingui/core';
import { useLingui } from '@lingui/react';

import { credentialAttentionQueryOptions } from '@/api/credential-profiles';
import type { CredentialAttention } from '@/api/schemas/credential-profiles';
import { getPlatformIcon } from '@/components/pipeline/constants';
import { NotificationBadge } from '@/components/ui/notification-badge';
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from '@/components/ui/popover';
import { formatPlatformName } from '@/lib/format';
import { formatRelativeTime } from '@/lib/date-utils';
import { usePresence } from '@/hooks/use-presence';
import {
  HEADER_BADGE_DOT,
  HEADER_POPOVER,
  HEADER_TRIGGER_EXIT_MS,
  headerTriggerClass,
} from '@/components/header-indicator';
import { cn } from '@/lib/utils';

/** Accounts grouped by platform, in the server's platform order. */
function groupByPlatform(accounts: CredentialAttention[]) {
  const groups = new Map<
    string,
    {
      platformId: string;
      platformName: string;
      accounts: CredentialAttention[];
    }
  >();
  for (const account of accounts) {
    const platformId = account.profile.platform_config_id;
    let group = groups.get(platformId);
    if (!group) {
      group = { platformId, platformName: account.platform_name, accounts: [] };
      groups.set(platformId, group);
    }
    group.accounts.push(account);
  }
  return [...groups.values()];
}

/** The header's summary: "need login" when that is all they need. */
export function attentionLabel(accounts: CredentialAttention[], i18n: I18n) {
  const count = accounts.length;
  return accounts.every((account) => account.reason === 'login_required')
    ? t(
        i18n,
      )`${plural(count, { one: '# account needs login', other: '# accounts need login' })}`
    : t(
        i18n,
      )`${plural(count, { one: '# account needs attention', other: '# accounts need attention' })}`;
}

function reasonLabel(account: CredentialAttention, i18n: I18n) {
  if (account.reason === 'login_required') {
    return i18n._(msg`Needs a new login`);
  }
  const failures = account.health.refresh_failure_count ?? 0;
  return t(
    i18n,
  )`${plural(failures, { one: 'Refresh failed # time', other: 'Refresh failed # times' })}`;
}

/**
 * Header entry for accounts that need the user: enabled accounts that only a
 * new login can fix, and rejected ones whose refresh keeps failing. Disabled
 * accounts are a choice and are not counted. Renders nothing while every
 * account is usable, or for a session that cannot read accounts.
 */
export function CredentialAttentionIndicator() {
  const { i18n } = useLingui();
  const [open, setOpen] = useState(false);
  const { data } = useQuery({
    ...credentialAttentionQueryOptions,
    // Accounts are read in the browser only; the live connection reports
    // changes, so there is no polling.
    enabled: typeof window !== 'undefined',
    retry: false,
  });
  const accounts = data ?? [];
  const hasContent = accounts.length > 0;
  const { mounted, exiting } = usePresence(hasContent, HEADER_TRIGGER_EXIT_MS);

  if (!mounted) return null;

  const label = attentionLabel(accounts, i18n);

  return (
    <Popover open={open && hasContent} onOpenChange={setOpen}>
      <PopoverTrigger
        aria-label={label}
        title={label}
        className={cn(
          headerTriggerClass(exiting),
          'text-amber-600 hover:text-amber-600 dark:text-amber-400 dark:hover:text-amber-400',
        )}
      >
        <KeyRound className="size-4" />
        <NotificationBadge
          open={hasContent}
          entering={hasContent}
          className="-top-0.5 -right-0.5"
          dotClassName={cn(HEADER_BADGE_DOT, 'bg-amber-500')}
        >
          {hasContent ? accounts.length : undefined}
        </NotificationBadge>
      </PopoverTrigger>
      <PopoverContent align="end" sideOffset={8} className={HEADER_POPOVER}>
        <div className="space-y-1 px-4 py-3">
          <p className="text-sm font-semibold">{label}</p>
          <p className="text-xs text-muted-foreground">
            <Trans>
              Streamers that select only these accounts stop recording until one
              works again.
            </Trans>
          </p>
        </div>
        <div className="max-h-80 overflow-y-auto border-t border-border/60 py-1">
          {groupByPlatform(accounts).map((group) => (
            <PlatformGroup
              key={group.platformId}
              {...group}
              onNavigate={() => setOpen(false)}
            />
          ))}
        </div>
      </PopoverContent>
    </Popover>
  );
}

function PlatformGroup({
  platformId,
  platformName,
  accounts,
  onNavigate,
}: {
  platformId: string;
  platformName: string;
  accounts: CredentialAttention[];
  onNavigate: () => void;
}) {
  const { i18n } = useLingui();
  const Icon = getPlatformIcon(platformName);
  const label = formatPlatformName(platformName);
  return (
    <section aria-label={label}>
      <div className="flex items-center gap-1.5 px-4 pt-2 pb-1 text-[11px] font-medium tracking-wide text-muted-foreground uppercase">
        <Icon className="size-3" />
        {label}
      </div>
      <ul>
        {accounts.map((account) => {
          const lastCheck = account.health.last_check_at;
          return (
            <li key={account.profile.id}>
              <Link
                to="/config/platforms/$platformId"
                params={{ platformId }}
                onClick={onNavigate}
                className="flex min-w-0 flex-col px-4 py-2 hover:bg-accent/60"
              >
                <span className="flex items-center gap-2">
                  <span className="min-w-0 truncate text-sm font-medium">
                    {account.profile.label}
                  </span>
                  <span className="ml-auto shrink-0 text-xs font-medium text-amber-600 dark:text-amber-400">
                    {reasonLabel(account, i18n)}
                  </span>
                </span>
                <span className="mt-0.5 truncate text-[11px] text-muted-foreground">
                  {lastCheck != null
                    ? i18n._(
                        msg`Last checked ${formatRelativeTime(lastCheck, i18n.locale)}`,
                      )
                    : i18n._(msg`Not checked yet`)}
                </span>
              </Link>
            </li>
          );
        })}
      </ul>
    </section>
  );
}
