import { memo, useMemo } from 'react';
import { useQuery } from '@tanstack/react-query';
import { Activity, Check, Clock, Radio } from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { msg } from '@lingui/core/macro';

import { cn } from '@/lib/utils';
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from '@/components/ui/tooltip';
import { useWebSocket } from '@/providers/WebSocketContext';
import {
  getStreamerCheckHistory,
  type StreamerCheckHistoryEntry,
} from '@/server/functions/streamers';

/**
 * Number of bar slots rendered. Matches the screenshot's "HISTORY (60PTS)"
 * UI. The server caps `?limit=` at 200 and the writer trims to 200 rows
 * per streamer; the strip slices to the most-recent 60 if more come back.
 */
const SLOTS = 60;

/** How often to refetch from REST. With WebSocket push wired, this is a
 *  fallback for missed events — broadcast lag/closed channels, brief WS
 *  disconnects on the dashboard tab, etc. Set conservatively (5 min);
 *  live updates flow through `WebSocketProvider`'s
 *  `STREAMER_CHECK_RECORDED` handler the rest of the time. */
const REFETCH_MS = 5 * 60_000;

interface StatusCheckHistoryProps {
  streamerId: string;
  /** Optional override — defaults to {@link SLOTS} (60). */
  slots?: number;
}

export const StatusCheckHistory = memo(function StatusCheckHistory({
  streamerId,
  slots = SLOTS,
}: StatusCheckHistoryProps) {
  const { i18n } = useLingui();
  const { isConnected } = useWebSocket();

  const { data, isLoading } = useQuery({
    queryKey: ['streamer', streamerId, 'check-history', slots],
    queryFn: () =>
      getStreamerCheckHistory({ data: { id: streamerId, limit: slots } }),
    refetchInterval: REFETCH_MS,
    refetchOnWindowFocus: true,
  });

  const items = data?.items ?? [];

  // Right-align bars so the most-recent check is at "NOW" (right edge).
  // Pad the left with empty placeholders when fewer than `slots` rows exist.
  const padded = useMemo(() => {
    const recent = items.slice(-slots);
    const padCount = Math.max(0, slots - recent.length);
    return [
      ...Array.from({ length: padCount }, () => null as null),
      ...recent,
    ] as Array<StreamerCheckHistoryEntry | null>;
  }, [items, slots]);

  // Latest row's timestamp, used to caption the relative-time hint.
  const lastChecked = useMemo(
    () =>
      items.length > 0
        ? new Date(items[items.length - 1].checked_at).getTime()
        : null,
    [items],
  );

  return (
    <div className="p-6 rounded-xl border bg-card/50 shadow-sm space-y-3">
      {/* `flex-wrap` so the right-side indicator drops below the label on
          narrow widths (sidebar collapsed, mobile) instead of forcing both
          to wrap mid-word. `min-w-0` + `whitespace-nowrap` on each child
          keeps each label as an atomic unit. */}
      <div className="flex flex-wrap items-center justify-between gap-x-3 gap-y-1">
        <div className="flex items-center gap-2 font-semibold text-xs uppercase tracking-wider text-muted-foreground whitespace-nowrap">
          <Activity className="w-4 h-4 shrink-0" />
          <Trans>History ({slots} pts)</Trans>
        </div>
        <LiveIndicator
          isConnected={isConnected}
          lastCheckedMs={lastChecked}
          i18n={i18n}
        />
      </div>

      <TooltipProvider delayDuration={150}>
        {/* `min-w-0` lets the strip shrink below its content's intrinsic
            width when the parent column is narrow (sidebar collapsed,
            mobile). Each bar uses `flex-1 basis-0 min-w-0` so 60 children
            distribute evenly without a hard pixel floor — bars compress to
            sub-pixel widths cleanly rather than overflowing the column.
            `overflow-hidden` clips the rounded right edge as a final
            guard. */}
        <div
          className="flex items-stretch gap-[1.5px] h-12 w-full min-w-0 overflow-hidden"
          aria-label={i18n._(msg`Streamer check history`)}
        >
          {padded.map((entry, idx) =>
            entry ? (
              // Stable key on the entry's timestamp alone — when a new
              // record arrives we right-align bars, so positional idx
              // shifts. Keying on `idx` would unmount/remount every
              // existing bar (and reset its tooltip state) on every
              // arrival; keying on `checked_at` keeps each bar's
              // identity stable across the shift. The writer guarantees
              // one row per poll so the timestamp is unique.
              <Bar key={entry.checked_at} entry={entry} i18n={i18n} />
            ) : (
              <div
                key={`empty-${idx}`}
                className="flex-1 basis-0 min-w-0 rounded-sm bg-muted/30 border border-dashed border-muted-foreground/10"
                aria-hidden
              />
            ),
          )}
        </div>
      </TooltipProvider>

      <div className="flex justify-between text-[10px] tracking-wider text-muted-foreground/70 uppercase">
        <span>
          <Trans>Past</Trans>
        </span>
        {isLoading && items.length === 0 && (
          <span>
            <Trans>Loading…</Trans>
          </span>
        )}
        <span>
          <Trans>Now</Trans>
        </span>
      </div>
    </div>
  );
});

interface BarProps {
  entry: StreamerCheckHistoryEntry;
  i18n: ReturnType<typeof useLingui>['i18n'];
}

const Bar = memo(function Bar({ entry, i18n }: BarProps) {
  const { color, height } = useMemo(
    () => barAppearance(entry.outcome),
    [entry.outcome],
  );

  // Localized aria-label: outcome name + timestamp. The raw string
  // `entry.outcome` is the wire-format discriminator (`live`, `offline`,
  // …) which must not reach the screen-reader as-is.
  const ariaLabel = i18n._(
    msg`${outcomeAriaName(entry.outcome, i18n)} at ${i18n.date(new Date(entry.checked_at), { timeStyle: 'short' })}`,
  );

  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <button
          type="button"
          aria-label={ariaLabel}
          className={cn(
            'flex-1 basis-0 min-w-0 rounded-sm transition-opacity hover:opacity-80 focus:outline-none focus-visible:ring-2 focus-visible:ring-ring',
            color,
          )}
          // Inline `height` keeps each outcome's relative height obvious;
          // promoting these to Tailwind tokens would just hide the mapping.
          style={{ height: `${height}%`, alignSelf: 'flex-end' }}
        />
      </TooltipTrigger>
      <TooltipContent
        side="top"
        className="max-w-xs space-y-1.5 px-3 py-2 text-xs"
      >
        <BarTooltipContent entry={entry} i18n={i18n} />
      </TooltipContent>
    </Tooltip>
  );
});

function BarTooltipContent({
  entry,
  i18n,
}: {
  entry: StreamerCheckHistoryEntry;
  i18n: ReturnType<typeof useLingui>['i18n'];
}) {
  const formatted = i18n.date(new Date(entry.checked_at), {
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
  });
  const selected = entry.stream_selected;
  const candidates = entry.streams_extracted_detail ?? [];

  return (
    <>
      <div className="flex items-center justify-between gap-3">
        <span className="font-medium">
          <OutcomeLabel outcome={entry.outcome} />
        </span>
        <span className="opacity-70">{formatted}</span>
      </div>
      {entry.title && (
        <div className="text-[11px] opacity-90 line-clamp-2">{entry.title}</div>
      )}

      {/* Stream candidates: when the persisted detail is present, list
          every candidate the extractor returned with the selected one
          marked with a check + emerald accent. Fall back to a count + the
          selected summary alone when the detail isn't available (older
          rows, or non-live outcomes). */}
      {candidates.length > 0 ? (
        <div className="flex flex-col gap-1">
          <div className="flex items-center justify-between gap-3 opacity-80">
            <span>
              <Trans>Streams extracted</Trans>
            </span>
            <span className="font-mono">{candidates.length}</span>
          </div>
          <ul className="font-mono text-[11px] space-y-0.5 max-h-40 overflow-y-auto">
            {candidates.map((c, idx) => (
              <CandidateRow
                key={idx}
                candidate={c}
                selected={isSameStream(c, selected)}
                i18n={i18n}
              />
            ))}
          </ul>
        </div>
      ) : (
        <>
          <div className="flex items-center justify-between gap-3 opacity-80">
            <span>
              <Trans>Streams extracted</Trans>
            </span>
            <span className="font-mono">{entry.streams_extracted}</span>
          </div>
          {selected && (
            <div className="flex flex-col gap-0.5">
              <span className="opacity-80">
                <Trans>Selected</Trans>
              </span>
              <span className="font-mono text-[11px]">
                {streamSummary(selected)}
              </span>
            </div>
          )}
        </>
      )}

      {entry.filter_reason && (
        <div className="opacity-80">
          <Trans>Filtered:</Trans>{' '}
          <span className="font-mono">{entry.filter_reason}</span>
        </div>
      )}
      {entry.fatal_kind && (
        <div className="opacity-80">
          <Trans>Fatal:</Trans>{' '}
          <span className="font-mono">{entry.fatal_kind}</span>
        </div>
      )}
      {entry.error_message && (
        <div className="opacity-80 line-clamp-3">
          <Trans>Error:</Trans>{' '}
          <span className="font-mono">{entry.error_message}</span>
        </div>
      )}
      <div className="flex items-center justify-between gap-3 opacity-60 text-[10px]">
        <span>
          <Trans>Check duration</Trans>
        </span>
        <span className="font-mono">{entry.duration_ms} ms</span>
      </div>
    </>
  );
}

type StreamSummary = NonNullable<StreamerCheckHistoryEntry['stream_selected']>;

function CandidateRow({
  candidate,
  selected,
  i18n,
}: {
  candidate: StreamSummary;
  selected: boolean;
  i18n: ReturnType<typeof useLingui>['i18n'];
}) {
  return (
    <li
      className={cn(
        'flex items-center gap-1.5',
        selected && 'text-emerald-400',
      )}
    >
      {selected ? (
        <Check
          className="w-3 h-3 shrink-0"
          aria-label={i18n._(msg`Selected stream`)}
        />
      ) : (
        // Fixed-width spacer so non-selected rows align with the
        // checkmark column instead of jumping leftward.
        <span className="w-3 shrink-0" aria-hidden />
      )}
      <span className={selected ? 'font-semibold' : 'opacity-80'}>
        {streamSummary(candidate)}
      </span>
    </li>
  );
}

/** Compact `quality · format · N kbps · codec@fps` line. Skips empty
 *  fields silently — some platforms leave bitrate/codec blank for HLS
 *  variants and we'd rather show what we have than nothing. */
function streamSummary(s: StreamSummary): string {
  return [
    s.quality,
    s.media_format,
    s.bitrate ? `${Math.round(s.bitrate / 1000)} kbps` : null,
    s.codec ? `${s.codec}${s.fps ? `@${s.fps}` : ''}` : null,
  ]
    .filter(Boolean)
    .join(' · ');
}

/** Two summaries are the same stream when their compact representations
 *  match — the selected descriptor and its position in the candidate list
 *  are produced from the same source data, so a string equality on the
 *  visible fields is sufficient and stable across (de)serialization. */
function isSameStream(
  a: StreamSummary,
  b: StreamSummary | null | undefined,
): boolean {
  if (!b) return false;
  return (
    a.quality === b.quality &&
    a.bitrate === b.bitrate &&
    a.codec === b.codec &&
    a.media_format === b.media_format &&
    a.stream_format === b.stream_format
  );
}

function OutcomeLabel({
  outcome,
}: {
  outcome: StreamerCheckHistoryEntry['outcome'];
}) {
  switch (outcome) {
    case 'live':
      return <Trans>Live</Trans>;
    case 'offline':
      return <Trans>Offline</Trans>;
    case 'filtered':
      return <Trans>Filtered</Trans>;
    case 'transient_error':
      return <Trans>Transient error</Trans>;
    case 'fatal_error':
      return <Trans>Fatal error</Trans>;
  }
}

/** Localized outcome name for screen-reader aria-labels. Uses the same
 *  message keys as `OutcomeLabel` so a single translation entry covers
 *  both the visible tooltip header and the assistive-tech announcement. */
function outcomeAriaName(
  outcome: StreamerCheckHistoryEntry['outcome'],
  i18n: ReturnType<typeof useLingui>['i18n'],
): string {
  switch (outcome) {
    case 'live':
      return i18n._(msg`Live`);
    case 'offline':
      return i18n._(msg`Offline`);
    case 'filtered':
      return i18n._(msg`Filtered`);
    case 'transient_error':
      return i18n._(msg`Transient error`);
    case 'fatal_error':
      return i18n._(msg`Fatal error`);
  }
}

/**
 * Per-outcome bar appearance. Color tokens follow the rest of the dashboard
 * (emerald = live, amber = filtered, red = error). Heights skew the eye
 * toward anomalies — short gray bars for offline polls, tall colored bars
 * everywhere else, matching the screenshot's visual rhythm.
 */
function barAppearance(outcome: StreamerCheckHistoryEntry['outcome']): {
  color: string;
  height: number;
} {
  switch (outcome) {
    case 'live':
      return { color: 'bg-emerald-500', height: 100 };
    case 'offline':
      return { color: 'bg-muted-foreground/40', height: 35 };
    case 'filtered':
      return { color: 'bg-amber-500', height: 100 };
    case 'transient_error':
      return { color: 'bg-red-500', height: 100 };
    case 'fatal_error':
      return { color: 'bg-red-700', height: 100 };
  }
}

/**
 * Header-right indicator. When the WebSocket is connected, render a "LIVE"
 * pulse — bars stream in via the broadcaster, so the strip is real-time.
 * When disconnected (server down, auth expired, brief reconnect window),
 * fall back to a relative-time caption sourced from the most-recent row,
 * so operators can tell whether they're looking at stale data.
 */
function LiveIndicator({
  isConnected,
  lastCheckedMs,
  i18n,
}: {
  isConnected: boolean;
  lastCheckedMs: number | null;
  i18n: ReturnType<typeof useLingui>['i18n'];
}) {
  if (isConnected) {
    return (
      <div className="flex items-center gap-1.5 text-xs text-emerald-500 whitespace-nowrap">
        <Radio className="w-3.5 h-3.5 animate-pulse shrink-0" />
        <span className="uppercase tracking-wider font-semibold">
          <Trans>Live</Trans>
        </span>
      </div>
    );
  }
  if (lastCheckedMs == null) {
    return (
      <div className="flex items-center gap-1.5 text-xs text-muted-foreground whitespace-nowrap">
        <Clock className="w-3.5 h-3.5 shrink-0" />
        <span>
          <Trans>Awaiting first check</Trans>
        </span>
      </div>
    );
  }
  const ago = Math.max(0, Math.round((Date.now() - lastCheckedMs) / 1000));
  return (
    <div className="flex items-center gap-1.5 text-xs text-muted-foreground whitespace-nowrap">
      <Clock className="w-3.5 h-3.5 shrink-0" />
      <span>{i18n._(msg`Last check ${formatAgo(ago, i18n)} ago`)}</span>
    </div>
  );
}

function formatAgo(
  seconds: number,
  i18n: ReturnType<typeof useLingui>['i18n'],
): string {
  if (seconds < 60) return i18n._(msg`${seconds}s`);
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return i18n._(msg`${minutes}m`);
  const hours = Math.round(minutes / 60);
  return i18n._(msg`${hours}h`);
}
