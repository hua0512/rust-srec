import type { ReactNode } from 'react';
import { useQuery } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { Trans } from '@lingui/react/macro';
import { platformAccountsQueryOptions } from '@/api/credential-profiles';
import { AccountListEmpty } from './account-list';
import { AccountRow } from './account-row';
import { ListPanel, ListPanelRows } from '@/components/shared/list-panel';

/**
 * The platform's accounts as a template or streamer sees them: the scope's own
 * `selection`, then the accounts with their health and usage, without
 * management, which happens on the platform page.
 */
export function ReadOnlyAccountsSection({
  platformId,
  selection,
}: {
  platformId: string;
  selection?: ReactNode;
}) {
  const accounts = useQuery(platformAccountsQueryOptions(platformId));
  return (
    <ListPanel
      title={<Trans>Accounts</Trans>}
      description={
        <>
          <Trans>
            Accounts belong to the platform. Add, edit or log in to them in the
            platform settings.
          </Trans>{' '}
          <Link
            to="/config/platforms/$platformId"
            params={{ platformId }}
            className="font-medium text-foreground/80 underline underline-offset-2 hover:text-foreground"
          >
            <Trans>Open platform settings</Trans>
          </Link>
        </>
      }
    >
      {selection}
      <ListPanelRows
        query={accounts}
        empty={
          <AccountListEmpty
            help={<Trans>Add them in the platform settings.</Trans>}
          />
        }
        renderRow={(detail) => (
          <AccountRow key={detail.profile.id} detail={detail} />
        )}
      />
    </ListPanel>
  );
}
