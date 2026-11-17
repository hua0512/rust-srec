import type { ReactNode } from 'react';
import { useQuery } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { Trans } from '@lingui/react/macro';
import { AlertTriangle } from 'lucide-react';

import { bigoNotificationStatusQueryOptions } from '@/api/platform-configs';
import type { BigoStreamerReason } from '@/api/schemas/platform-configs';
import { Callout } from '@/components/shared/callout';
import { getPlatformFromUrl } from '@/lib/platform';

/** Why Bigo live notifications are not used, for the listener or one streamer. */
export function bigoReasonLabel(reason: BigoStreamerReason): ReactNode {
  switch (reason) {
    case 'notifications_off':
      return <Trans>Live notifications are off</Trans>;
    case 'no_account':
      return <Trans>No notification account is chosen</Trans>;
    case 'account_unusable':
      return (
        <Trans>
          The notification account is missing, disabled or not a BIGO account
        </Trans>
      );
    case 'signed_out':
      return <Trans>The notification account needs a new login</Trans>;
    case 'disconnected':
      return <Trans>Not connected to BIGO</Trans>;
    case 'streamer_disabled':
      return <Trans>The streamer is disabled</Trans>;
    case 'batch_detection_off':
      return <Trans>Batch Detection is off for this streamer</Trans>;
    case 'streamlink_extractor':
      return <Trans>The streamer is checked with Streamlink</Trans>;
    case 'uid_unknown':
      return (
        <Trans>
          The streamer's BIGO account is not known yet; its next check finds it
        </Trans>
      );
    case 'subscribing':
      return <Trans>Waiting for BIGO to confirm the follow</Trans>;
    case 'follow_refused':
      return <Trans>BIGO refused to follow the streamer</Trans>;
    case 'unknown':
      return <Trans>Live notifications are not available</Trans>;
    default:
      return reason satisfies never;
  }
}

/**
 * The warning shown wherever Bigo streamers are polled. With `platformId`, its last sentence
 * links to the Bigo platform settings.
 */
export function BigoPollingWarning({
  platformId,
}: {
  platformId?: string | null;
}) {
  return (
    <Callout tone="warning" icon={AlertTriangle} role="status">
      <Trans>
        Bigo streamers are checked by polling, which is inconsistent: stream
        starts are noticed late, short streams can be missed, and some networks
        need an account for checks.
      </Trans>{' '}
      {platformId ? (
        <Link
          to="/config/platforms/$platformId"
          params={{ platformId }}
          className="font-medium underline underline-offset-2"
        >
          <Trans>Set up live notifications.</Trans>
        </Link>
      ) : (
        <Trans>Set up live notifications.</Trans>
      )}
    </Callout>
  );
}

/**
 * The polling warning for a streamer being added or edited, while its URL is a Bigo one and Bigo
 * live notifications are not connected. Decided from the listener alone: a streamer that is not
 * saved yet has no row of its own.
 */
export function BigoPollingNotice({ url }: { url: string | undefined }) {
  const isBigo = !!url && getPlatformFromUrl(url) === 'bigo';
  const { data: status } = useQuery({
    ...bigoNotificationStatusQueryOptions,
    enabled: isBigo,
  });
  if (!isBigo || !status || status.state === 'connected') return null;
  return <BigoPollingWarning platformId={status.platform_id} />;
}
