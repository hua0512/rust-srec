import type { ReactNode } from 'react';
import { useQuery } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { Trans } from '@lingui/react/macro';
import { AlertTriangle, BellOff } from 'lucide-react';

import { bigoNotificationStatusQueryOptions } from '@/api/platform-configs';
import type {
  BigoNotificationStatus,
  BigoStreamerReason,
} from '@/api/schemas/platform-configs';
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
    case 'notifications_not_arriving':
      return (
        <Trans>
          BIGO is not sending live notifications; polled until one arrives
        </Trans>
      );
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
        starts are noticed late, and short streams can be missed.
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

/** Polling reasons that pass on their own or make monitoring moot, which need no warning. */
const UNWARNED: ReadonlySet<BigoStreamerReason> = new Set([
  'subscribing',
  'streamer_disabled',
]);

/**
 * Why the streamer is polled, or `null` when it uses notifications. A saved streamer is judged by
 * its own row; one the status does not list yet, by the listener's state and reason, which while
 * connected is set only when BIGO sends no notifications.
 */
function pollingReason(
  status: BigoNotificationStatus,
  streamerId: string | undefined,
): BigoStreamerReason | null {
  const row = streamerId
    ? status.streamers.find((entry) => entry.streamer_id === streamerId)
    : undefined;
  if (row) return row.method === 'polling' ? (row.reason ?? 'unknown') : null;
  if (status.state === 'connected') return status.reason;
  return status.reason ?? 'unknown';
}

/**
 * The polling warning for a streamer being added or edited, while its URL is a Bigo one and the
 * streamer would be polled. A listener that is connecting warns about nothing yet.
 */
export function BigoPollingNotice({
  url,
  streamerId,
}: {
  url: string | undefined;
  streamerId?: string;
}) {
  const isBigo = !!url && getPlatformFromUrl(url) === 'bigo';
  const { data: status } = useQuery({
    ...bigoNotificationStatusQueryOptions,
    enabled: isBigo,
  });
  if (!isBigo || !status) return null;
  const reason = pollingReason(status, streamerId);
  if (
    !reason ||
    UNWARNED.has(reason) ||
    (reason === 'disconnected' && status.state === 'connecting')
  ) {
    return null;
  }
  return <BigoPollingWarning platformId={status.platform_id} />;
}

/**
 * The warning on the streamers page while Bigo live notifications have stopped working: BIGO sends
 * the account none, or the listener has stayed disconnected past the server's grace period for
 * brief drops. Without Bigo streamers on the page it shows nothing, without asking for the status.
 */
export function BigoNotificationsAlert({
  hasBigoStreamers,
  className,
}: {
  hasBigoStreamers: boolean;
  className?: string;
}) {
  const { data: status } = useQuery({
    ...bigoNotificationStatusQueryOptions,
    enabled: hasBigoStreamers,
  });
  if (!hasBigoStreamers || !status) return null;
  const notArriving = status.reason === 'notifications_not_arriving';
  if (!notArriving && !status.down_since) return null;
  const platformId = status.platform_id;
  return (
    <div className={className}>
      <Callout
        tone="warning"
        icon={BellOff}
        role="status"
        title={
          notArriving ? (
            <Trans>BIGO is not sending live notifications</Trans>
          ) : (
            <Trans>Bigo live notifications are disconnected</Trans>
          )
        }
        action={
          platformId && (
            <Link
              to="/config/platforms/$platformId"
              params={{ platformId }}
              className="font-medium underline underline-offset-2"
            >
              <Trans>Open the Bigo settings</Trans>
            </Link>
          )
        }
      >
        {notArriving ? (
          <Trans>
            Bigo streamers are polled until a notification arrives, so stream
            starts are noticed late. Check that the notification account is not
            signed in to the BIGO app and that live notifications are on in its
            BIGO settings.
          </Trans>
        ) : (
          <Trans>
            rust-srec keeps trying to reconnect. Meanwhile Bigo streamers are
            polled, so stream starts are noticed late.
          </Trans>
        )}
      </Callout>
    </div>
  );
}
