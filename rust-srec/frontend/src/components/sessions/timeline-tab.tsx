import { Card, CardContent } from '@/components/ui/card';
import { Badge } from '@/components/ui/badge';
import { useLingui } from '@lingui/react';
import { Trans } from '@lingui/react/macro';
import { motion } from 'motion/react';
import {
  Clock,
  Circle,
  ArrowDown,
  Hourglass,
  PlayCircle,
  RefreshCw,
  StopCircle,
} from 'lucide-react';
import { cn } from '@/lib/utils';
import type {
  SessionEvent,
  SessionEventPayload,
  TerminalCauseDto,
} from '@/api/schemas/session';

// --- Unified timeline entry model ---
//
// Title changes (from `session.titles`) and lifecycle events
// (`session.events`) live in two arrays on the API response. We merge them
// chronologically into a single render list so operators see "session
// started → title changed → entered hysteresis → resumed → ended" in one
// view. The discriminated union keeps per-kind rendering exhaustive.

type TitleEntry = { title: string; timestamp: string };

type TimelineEntry =
  | {
      kind: 'title';
      timestamp: string;
      title: string;
      previousTitle: string | null;
      isInitial: boolean;
    }
  | {
      kind: 'session_started';
      timestamp: string;
      fromHysteresis: boolean;
      title: string | null;
    }
  | {
      kind: 'hysteresis_entered';
      timestamp: string;
      cause: TerminalCauseDto;
      resumeDeadline: string;
    }
  | {
      kind: 'session_resumed';
      timestamp: string;
      hysteresisDurationSecs: number;
    }
  | {
      kind: 'session_ended';
      timestamp: string;
      cause: TerminalCauseDto;
      viaHysteresis: boolean;
    }
  | {
      // Fallback for an unrecognised event kind (forward compat — backend
      // adds a new variant before the frontend ships an updater).
      kind: 'unknown_event';
      timestamp: string;
      rawKind: string;
    };

function buildEntries(
  titles: TitleEntry[],
  events: SessionEvent[],
): TimelineEntry[] {
  const entries: TimelineEntry[] = [];

  titles.forEach((t, i) => {
    entries.push({
      kind: 'title',
      timestamp: t.timestamp,
      title: t.title,
      previousTitle: i > 0 ? titles[i - 1].title : null,
      isInitial: i === 0,
    });
  });

  for (const e of events) {
    const payload = e.payload as SessionEventPayload | null | undefined;
    if (!payload) {
      entries.push({
        kind: 'unknown_event',
        timestamp: e.occurred_at,
        rawKind: e.kind,
      });
      continue;
    }
    switch (payload.kind) {
      case 'session_started':
        entries.push({
          kind: 'session_started',
          timestamp: e.occurred_at,
          fromHysteresis: payload.from_hysteresis,
          title: payload.title ?? null,
        });
        break;
      case 'hysteresis_entered':
        entries.push({
          kind: 'hysteresis_entered',
          timestamp: e.occurred_at,
          cause: payload.cause,
          resumeDeadline: payload.resume_deadline,
        });
        break;
      case 'session_resumed':
        entries.push({
          kind: 'session_resumed',
          timestamp: e.occurred_at,
          hysteresisDurationSecs: payload.hysteresis_duration_secs,
        });
        break;
      case 'session_ended':
        entries.push({
          kind: 'session_ended',
          timestamp: e.occurred_at,
          cause: payload.cause,
          viaHysteresis: payload.via_hysteresis,
        });
        break;
    }
  }

  // Stable chronological sort. `Date.parse` returns NaN for malformed
  // strings; coerce to 0 so the entry sinks to the top rather than tripping
  // an exception. The backend always emits ISO-8601 timestamps, so this
  // only matters for hand-crafted test data.
  entries.sort((a, b) => {
    const ta = Date.parse(a.timestamp) || 0;
    const tb = Date.parse(b.timestamp) || 0;
    return ta - tb;
  });

  return entries;
}

// --- Cause rendering ---

function TerminalCauseLabel({ cause }: { cause: TerminalCauseDto }) {
  // `context="terminal-cause"` keeps these translations independent of the
  // same English words used elsewhere (e.g. "Completed" for finished
  // pipeline jobs at routes/.../pipeline/jobs/index.lazy.tsx). The
  // semantics here are engine-level disconnect/abort reasons, not
  // user-facing success states.
  switch (cause.type) {
    case 'completed':
      return <Trans context="terminal-cause">Completed</Trans>;
    case 'failed':
      return <Trans context="terminal-cause">Failed</Trans>;
    case 'cancelled':
      return <Trans context="terminal-cause">Cancelled</Trans>;
    case 'rejected':
      return <Trans context="terminal-cause">Rejected</Trans>;
    case 'streamer_offline':
      return <Trans context="terminal-cause">Streamer Offline</Trans>;
    case 'definitive_offline':
      return <DefinitiveOfflineLabel signalType={cause.signal.type} />;
  }
}

function DefinitiveOfflineLabel({ signalType }: { signalType: string }) {
  switch (signalType) {
    case 'danmu_stream_closed':
      return <Trans context="terminal-cause">Danmu Stream Closed</Trans>;
    case 'playlist_gone':
      return <Trans context="terminal-cause">Playlist Gone</Trans>;
    case 'consecutive_failures':
      return <Trans context="terminal-cause">Consecutive Failures</Trans>;
    default:
      return <Trans context="terminal-cause">Definitive Offline</Trans>;
  }
}

// --- Visuals per kind ---

function entryNodeIcon(entry: TimelineEntry) {
  switch (entry.kind) {
    case 'title':
      return (
        <Circle className="w-3 h-3 text-muted-foreground fill-muted-foreground/50" />
      );
    case 'session_started':
      return <PlayCircle className="w-3.5 h-3.5 text-emerald-500" />;
    case 'hysteresis_entered':
      return <Hourglass className="w-3.5 h-3.5 text-amber-500" />;
    case 'session_resumed':
      return <RefreshCw className="w-3.5 h-3.5 text-blue-500" />;
    case 'session_ended':
      return <StopCircle className="w-3.5 h-3.5 text-rose-500" />;
    case 'unknown_event':
      return <Circle className="w-3 h-3 text-muted-foreground" />;
  }
}

function entryBadge(entry: TimelineEntry) {
  switch (entry.kind) {
    case 'title':
      return entry.isInitial ? (
        <Badge
          variant="secondary"
          className="text-[10px] tracking-wider font-normal"
        >
          <Trans>INITIAL</Trans>
        </Badge>
      ) : (
        <Badge
          variant="outline"
          className="text-[10px] tracking-wider font-normal"
        >
          <Trans>UPDATE</Trans>
        </Badge>
      );
    case 'session_started':
      return (
        <Badge
          variant="secondary"
          className="text-[10px] tracking-wider font-normal bg-emerald-500/10 text-emerald-600 dark:text-emerald-400"
        >
          {entry.fromHysteresis ? (
            <Trans>RESUMED START</Trans>
          ) : (
            <Trans>SESSION STARTED</Trans>
          )}
        </Badge>
      );
    case 'hysteresis_entered':
      return (
        <Badge
          variant="outline"
          className="text-[10px] tracking-wider font-normal bg-amber-500/10 text-amber-600 dark:text-amber-400 border-amber-500/30"
        >
          <Trans>PENDING CONFIRMATION</Trans>
        </Badge>
      );
    case 'session_resumed':
      return (
        <Badge
          variant="outline"
          className="text-[10px] tracking-wider font-normal bg-blue-500/10 text-blue-600 dark:text-blue-400 border-blue-500/30"
        >
          <Trans>RESUMED</Trans>
        </Badge>
      );
    case 'session_ended':
      return (
        <Badge
          variant="outline"
          className="text-[10px] tracking-wider font-normal bg-rose-500/10 text-rose-600 dark:text-rose-400 border-rose-500/30"
        >
          <Trans>SESSION ENDED</Trans>
        </Badge>
      );
    case 'unknown_event':
      return (
        <Badge
          variant="outline"
          className="text-[10px] tracking-wider font-normal"
        >
          {entry.rawKind.toUpperCase()}
        </Badge>
      );
  }
}

function EntryBody({ entry }: { entry: TimelineEntry }) {
  switch (entry.kind) {
    case 'title':
      return (
        <>
          <div className="font-medium text-base leading-snug text-foreground">
            {entry.title}
          </div>
          {entry.previousTitle !== null && (
            <div className="mt-4 pt-3 border-t border-border/30">
              <div className="text-[10px] uppercase tracking-widest text-muted-foreground/50 mb-1.5">
                <Trans>Previous Title</Trans>
              </div>
              <div className="text-sm text-muted-foreground line-through decoration-destructive/30 decoration-1">
                {entry.previousTitle}
              </div>
            </div>
          )}
        </>
      );
    case 'session_started':
      return (
        <div className="text-sm text-foreground">
          {entry.title ? (
            <span className="font-medium">{entry.title}</span>
          ) : (
            <span className="text-muted-foreground italic">
              <Trans>Recording started</Trans>
            </span>
          )}
        </div>
      );
    case 'hysteresis_entered':
      return (
        <div className="text-sm space-y-1">
          <div className="text-foreground">
            <Trans>Cause:</Trans>{' '}
            <span className="font-medium">
              <TerminalCauseLabel cause={entry.cause} />
            </span>
          </div>
        </div>
      );
    case 'session_resumed':
      return (
        <div className="text-sm text-foreground">
          <Trans>
            Resumed after {entry.hysteresisDurationSecs}s in pending state.
          </Trans>
        </div>
      );
    case 'session_ended':
      return (
        <div className="text-sm space-y-1">
          <div className="text-foreground">
            <Trans>Cause:</Trans>{' '}
            <span className="font-medium">
              <TerminalCauseLabel cause={entry.cause} />
            </span>
          </div>
          {entry.viaHysteresis && (
            <div className="text-xs text-muted-foreground">
              <Trans>Confirmed via backstop timer.</Trans>
            </div>
          )}
        </div>
      );
    case 'unknown_event':
      return (
        <div className="text-sm text-muted-foreground italic">
          <Trans>Unrecognised event kind.</Trans>
        </div>
      );
  }
}

interface TimelineTabProps {
  session: any;
}

export function TimelineTab({ session }: TimelineTabProps) {
  const { i18n } = useLingui();
  const titles: TitleEntry[] = session.titles ?? [];
  const events: SessionEvent[] = session.events ?? [];
  const entries = buildEntries(titles, events);

  return (
    <motion.div
      initial={{ opacity: 0, y: 10 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ delay: 0.2 }}
      className="max-w-4xl mx-auto py-8"
    >
      {entries.length === 0 ? (
        <div className="text-center py-20 text-muted-foreground flex flex-col items-center gap-6">
          <div className="h-24 w-24 bg-muted/30 rounded-full flex items-center justify-center border border-border/50">
            <Clock className="h-10 w-10 opacity-20" />
          </div>
          <div className="space-y-1">
            <p className="text-lg font-medium text-foreground">
              <Trans>No Timeline Events</Trans>
            </p>
            <p className="text-sm">
              <Trans>
                No title changes or lifecycle events were recorded for this
                session.
              </Trans>
            </p>
          </div>
        </div>
      ) : (
        <div className="relative">
          {/* Central Line */}
          <div className="absolute left-8 md:left-1/2 top-4 bottom-4 w-px bg-gradient-to-b from-transparent via-border/60 to-transparent -translate-x-1/2" />

          <div className="space-y-12">
            {entries.map((entry, i) => {
              const isLast = i === entries.length - 1;
              const isLive = isLast && entry.kind !== 'session_ended';

              return (
                <motion.div
                  key={`${entry.kind}-${entry.timestamp}-${i}`}
                  initial={{ opacity: 0, y: 20 }}
                  animate={{ opacity: 1, y: 0 }}
                  transition={{ delay: i * 0.1 }}
                  className={cn(
                    'relative flex flex-col md:flex-row gap-8 md:gap-0 items-start md:items-center group',
                    i % 2 === 0 ? 'md:flex-row-reverse' : '',
                  )}
                >
                  {/* Timeline Node */}
                  <div className="absolute left-8 md:left-1/2 -translate-x-1/2 flex flex-col items-center justify-center">
                    <div
                      className={cn(
                        'relative z-10 flex items-center justify-center w-8 h-8 rounded-full border-4 border-background transition-transform duration-300 group-hover:scale-110 shadow-sm',
                        isLive
                          ? 'bg-primary border-primary/20 ring-4 ring-primary/10'
                          : 'bg-card border-border',
                      )}
                    >
                      {isLive ? (
                        <div className="h-2.5 w-2.5 bg-background rounded-full animate-pulse" />
                      ) : (
                        entryNodeIcon(entry)
                      )}
                    </div>
                  </div>

                  {/* Date/Time Marker (Desktop) */}
                  <div
                    className={cn(
                      'hidden md:flex w-1/2 px-12 items-center text-sm text-muted-foreground/60 font-mono',
                      i % 2 === 0 ? 'justify-start' : 'justify-end',
                    )}
                  >
                    {i18n.date(new Date(entry.timestamp), {
                      hour: 'numeric',
                      minute: 'numeric',
                      second: 'numeric',
                    })}
                  </div>

                  {/* Content Card */}
                  <div className={cn('w-full md:w-1/2 pl-20 md:pl-0 md:px-12')}>
                    <Card className="bg-card/40 backdrop-blur-sm border-border/40 hover:border-primary/20 hover:bg-card/60 transition-all duration-300 group-hover:shadow-lg relative overflow-hidden">
                      {/* Mobile Time Stamp */}
                      <div className="md:hidden absolute top-3 right-3 text-[10px] font-mono text-muted-foreground/60 bg-muted/30 px-1.5 py-0.5 rounded">
                        {i18n.date(new Date(entry.timestamp), {
                          hour: 'numeric',
                          minute: 'numeric',
                        })}
                      </div>

                      <CardContent className="p-5">
                        <div className="flex flex-col gap-1">
                          <div className="flex items-center gap-2 mb-2">
                            {entryBadge(entry)}
                          </div>
                          <EntryBody entry={entry} />
                        </div>
                      </CardContent>
                    </Card>
                  </div>
                </motion.div>
              );
            })}

            {/* End Node */}
            <div className="relative flex justify-center py-4">
              <div className="absolute left-8 md:left-1/2 -translate-x-1/2 w-px h-8 bg-gradient-to-b from-border/60 to-transparent -top-8" />
              <div className="md:ml-auto md:mr-auto ml-8 -translate-x-1/2 md:translate-x-0 bg-muted/20 text-[10px] text-muted-foreground px-3 py-1 rounded-full border border-border/20 flex items-center gap-2">
                <ArrowDown className="h-3 w-3" />
                <Trans>End of History</Trans>
              </div>
            </div>
          </div>
        </div>
      )}
    </motion.div>
  );
}
