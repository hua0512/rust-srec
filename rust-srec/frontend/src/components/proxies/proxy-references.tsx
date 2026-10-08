import type { ReactNode } from 'react';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { KeyRound, LayoutTemplate } from 'lucide-react';
import { formatPlatformName } from '@/lib/format';
import { cn } from '@/lib/utils';
import type { ProxyReferences } from '@/api/schemas/proxies';
import { NameList } from '@/components/shared/list-panel';
import { NAME_ICON, OwnerLink, TERM } from '@/components/shared/owner-link';

type Template = ProxyReferences['templates'][number];
type Account = ProxyReferences['accounts'][number];

function TemplateName({ template }: { template: Template }) {
  const { i18n } = useLingui();
  if (template.being_removed)
    return (
      <span
        className="min-w-0 truncate text-muted-foreground"
        title={i18n._(msg`Deleted; kept until the recordings using it finish`)}
      >
        <LayoutTemplate className={NAME_ICON} />
        {template.name}{' '}
        <span className="italic">
          <Trans>(being removed)</Trans>
        </span>
      </span>
    );
  return (
    <OwnerLink owner={{ type: 'template', id: template.id }}>
      {template.name}
    </OwnerLink>
  );
}

function AccountName({ account }: { account: Account }) {
  const platform = formatPlatformName(account.platform_name);
  return (
    <OwnerLink
      owner={{ type: 'platform', id: account.platform_id }}
      icon={KeyRound}
    >
      {account.label}{' '}
      <span className="font-normal text-muted-foreground">({platform})</span>
    </OwnerLink>
  );
}

/**
 * Every setting whose route names a proxy, grouped by kind, each linking to
 * where it is changed. `limit` caps the names shown per group.
 */
export function ProxyReferencesList({
  references,
  limit,
  className,
}: {
  references: ProxyReferences;
  limit?: number;
  className?: string;
}) {
  const { global, platforms, templates, streamers, accounts } = references;
  const rows: { term: ReactNode; value: ReactNode }[] = [];
  const cap = (count: number) => limit ?? count;
  if (global)
    rows.push({
      term: <Trans>Global</Trans>,
      value: (
        <OwnerLink owner={{ type: 'global' }}>
          <Trans>Global settings</Trans>
        </OwnerLink>
      ),
    });
  if (platforms.length)
    rows.push({
      term: <Trans>Platforms</Trans>,
      value: (
        <NameList
          items={platforms}
          limit={cap(platforms.length)}
          label={(platform) => formatPlatformName(platform.name)}
          render={(platform) => (
            <OwnerLink owner={{ type: 'platform', id: platform.id }}>
              {formatPlatformName(platform.name)}
            </OwnerLink>
          )}
        />
      ),
    });
  if (templates.length)
    rows.push({
      term: <Trans>Templates</Trans>,
      value: (
        <NameList
          items={templates}
          limit={cap(templates.length)}
          label={(template) => template.name}
          render={(template) => <TemplateName template={template} />}
        />
      ),
    });
  if (streamers.length)
    rows.push({
      term: <Trans>Streamers</Trans>,
      value: (
        <NameList
          items={streamers}
          limit={cap(streamers.length)}
          label={(streamer) => streamer.name}
          render={(streamer) => (
            <OwnerLink owner={{ type: 'streamer', id: streamer.id }}>
              {streamer.name}
            </OwnerLink>
          )}
        />
      ),
    });
  if (accounts.length)
    rows.push({
      term: <Trans context="proxy references">Accounts</Trans>,
      value: (
        <NameList
          items={accounts}
          limit={cap(accounts.length)}
          label={(account) => account.label}
          render={(account) => <AccountName account={account} />}
        />
      ),
    });
  if (!rows.length)
    return (
      <p className={cn('text-muted-foreground/80', className)}>
        <Trans>Not used anywhere</Trans>
      </p>
    );
  return (
    <dl
      className={cn(
        'grid grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-1.5',
        className,
      )}
    >
      {rows.map((row, index) => (
        <div key={index} className="contents">
          <dt className={TERM}>{row.term}</dt>
          <dd>{row.value}</dd>
        </div>
      ))}
    </dl>
  );
}
