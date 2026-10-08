import type { ReactNode } from 'react';
import { Trans } from '@lingui/react/macro';
import { KeyRound } from 'lucide-react';
import { ListPanelEmpty } from '@/components/shared/list-panel';

/** What a platform without accounts shows in place of its rows. */
export function AccountListEmpty({
  help,
  action,
}: {
  help: ReactNode;
  action?: ReactNode;
}) {
  return (
    <ListPanelEmpty
      icon={KeyRound}
      title={<Trans>No accounts yet</Trans>}
      help={help}
      action={action}
    />
  );
}
