import { Link } from '@tanstack/react-router';
import { Trans } from '@lingui/react/macro';
import { useQuery } from '@tanstack/react-query';
import { BellRing, Radar } from 'lucide-react';

import { bigoNotificationStatusQueryOptions } from '@/api/platform-configs';
import type {
  BigoNotificationStatus,
  BigoStreamerReason,
} from '@/api/schemas/platform-configs';
import type { Streamer } from '@/api/schemas/streamer';
import { bigoReasonLabel } from '@/components/shared/bigo-notifications';
import { StatusInfoTooltip } from '@/components/shared/status-info-tooltip';
import { getPlatformFromUrl } from '@/lib/platform';
import { StatusPill } from './stream-status-badge';

/** How a Bigo streamer is monitored, and why when it is polled. */
export interface MonitoringMethod {
  method: 'notifications' | 'polling';
  reason: BigoStreamerReason | null;
  /** The Bigo platform, for a link to its settings. */
  platformId: string | null;
}

/**
 * The streamer's row of the shared Bigo notification status. A streamer the status does not list
 * yet (added since it was read) is polled for the listener's reason.
 */
function monitoringMethodOf(
  status: BigoNotificationStatus,
  streamerId: string,
): MonitoringMethod {
  const row = status.streamers.find(
    (entry) => entry.streamer_id === streamerId,
  );
  return row
    ? { method: row.method, reason: row.reason, platformId: status.platform_id }
    : {
        method: 'polling',
        reason: status.reason ?? 'unknown',
        platformId: status.platform_id,
      };
}

/**
 * How a Bigo streamer is monitored: live notifications, or polling. `null` until the status has
 * loaded. Every caller shares one query.
 */
export function useMonitoringMethod(
  streamerId: string,
): MonitoringMethod | null {
  const { data } = useQuery({
    ...bigoNotificationStatusQueryOptions,
    select: (status) => monitoringMethodOf(status, streamerId),
  });
  return data ?? null;
}

/**
 * The monitoring method beside an enabled Bigo streamer's status, with what it means. Other
 * streamers get nothing, without asking for the status.
 */
export function MonitoringMethodBadge({
  streamer,
}: {
  streamer: Pick<Streamer, 'id' | 'url' | 'enabled'>;
}) {
  if (!streamer.enabled || getPlatformFromUrl(streamer.url) !== 'bigo') {
    return null;
  }
  return <BigoMethodBadge streamerId={streamer.id} />;
}

function BigoMethodBadge({ streamerId }: { streamerId: string }) {
  const method = useMonitoringMethod(streamerId);
  if (!method) return null;
  return method.method === 'notifications' ? (
    <StatusPill
      className="gap-1 bg-emerald-500/10 text-emerald-600 border-emerald-500/20 dark:text-emerald-400 dark:border-emerald-400/30"
      tooltip={
        <StatusInfoTooltip
          theme="blue"
          icon={<BellRing className="h-3.5 w-3.5" />}
          title={<Trans>Live notifications</Trans>}
        >
          <div className="text-xs text-muted-foreground leading-relaxed">
            <Trans>
              BIGO tells rust-srec when this streamer goes live, and it is
              checked at once. Offline checks need no request of their own,
              apart from a real check at least every 10 minutes.
            </Trans>
          </div>
        </StatusInfoTooltip>
      }
    >
      <BellRing className="h-3 w-3" aria-hidden="true" />
      <Trans context="monitoring method">Notifications</Trans>
    </StatusPill>
  ) : (
    <StatusPill
      className="gap-1 bg-amber-500/10 text-amber-600 border-amber-500/20 dark:text-amber-400 dark:border-amber-400/30"
      tooltip={
        <StatusInfoTooltip
          theme="amber"
          icon={<Radar className="h-3.5 w-3.5" />}
          title={<Trans context="monitoring method">Polling</Trans>}
          subtitle={method.reason && bigoReasonLabel(method.reason)}
        >
          <div className="text-xs text-muted-foreground leading-relaxed">
            <Trans>
              Every check asks BIGO whether the streamer is live. A stream start
              is noticed up to one check interval late, so short streams can be
              missed.
            </Trans>
          </div>
          {method.platformId && (
            <Link
              to="/config/platforms/$platformId"
              params={{ platformId: method.platformId }}
              className="inline-flex items-center gap-1.5 text-xs font-medium text-amber-700 hover:underline dark:text-amber-400"
            >
              <BellRing className="h-3 w-3" />
              <Trans>Set up live notifications</Trans>
            </Link>
          )}
        </StatusInfoTooltip>
      }
    >
      <Radar className="h-3 w-3" aria-hidden="true" />
      <Trans context="monitoring method">Polling · inconsistent</Trans>
    </StatusPill>
  );
}
