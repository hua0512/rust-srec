import {
  type ReactNode,
  memo,
  useCallback,
  useEffect,
  useMemo,
  useReducer,
  useRef,
  useState,
} from 'react';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { msg, plural, t } from '@lingui/core/macro';
import { RefreshCw, Terminal } from 'lucide-react';
import { Badge } from '@/components/ui/badge';
import { Card } from '@/components/ui/card';
import { useInView } from '@/hooks/use-in-view';
import { getDateFormatter } from '@/lib/datetime';
import {
  LOG_CHUNK_SIZE,
  flattenLogPages,
  latestLogTotal,
  sameLogRows,
  visibleLogChunks,
  type JobLogEntry,
  type JobLogPages,
} from '@/lib/job-log-pages';
import { cn } from '@/lib/utils';

/** One unwrapped row: a `text-xs` line, its padding and the gap below it. */
const ESTIMATED_ROW_PX = 24;
/** Rows kept mounted beyond each edge of the viewport, as a distance. */
const OVERSCAN_PX = 800;
const TIME_OPTIONS: Intl.DateTimeFormatOptions = { timeStyle: 'medium' };

interface JobLogViewerProps {
  data: JobLogPages | undefined;
  isLoading: boolean;
  hasNextPage: boolean;
  isFetchingNextPage: boolean;
  fetchNextPage: () => unknown;
  errorCount?: number | null;
  warnCount?: number | null;
  linesTotal?: number | null;
}

/**
 * The job's execution log. Memoized on the log data alone, so the page's
 * progress and status polls do not re-render it, and only the rows near the
 * viewport are mounted: a long job holds thousands of rows across its pages.
 */
export const JobLogViewer = memo(function JobLogViewer({
  data,
  isLoading,
  hasNextPage,
  isFetchingNextPage,
  fetchNextPage,
  errorCount,
  warnCount,
  linesTotal,
}: JobLogViewerProps) {
  const { i18n } = useLingui();
  const logs = useMemo(() => flattenLogPages(data), [data]);
  const totalLogs = latestLogTotal(data);
  const timeFormat = getDateFormatter(i18n.locale, TIME_OPTIONS);

  const { ref: loadMoreRef, inView } = useInView();
  useEffect(() => {
    if (inView && hasNextPage) {
      void fetchNextPage();
    }
  }, [inView, hasNextPage, fetchNextPage]);

  return (
    <Card className="border-border/40 bg-zinc-950 shadow-2xl overflow-hidden rounded-xl">
      <div className="flex items-center justify-between px-4 py-3 bg-zinc-900/80 border-b border-zinc-800 backdrop-blur-md">
        <div className="flex items-center gap-3">
          <div className="flex gap-1.5">
            <div className="w-3 h-3 rounded-full bg-red-500/20 border border-red-500/50" />
            <div className="w-3 h-3 rounded-full bg-yellow-500/20 border border-yellow-500/50" />
            <div className="w-3 h-3 rounded-full bg-green-500/20 border border-green-500/50" />
          </div>
          <div className="h-4 w-px bg-zinc-700 mx-1" />
          <span className="text-zinc-400 text-xs font-mono font-medium flex items-center gap-2">
            <Terminal className="h-3.5 w-3.5" /> execution-logs.log
          </span>
        </div>
        <div className="flex items-center gap-2">
          {(errorCount ?? 0) > 0 && (
            <Badge
              variant="outline"
              className="text-[10px] border-red-500/30 text-red-400 bg-red-500/10 hover:bg-red-500/20 transition-colors"
            >
              {t(
                i18n,
              )`${plural(errorCount ?? 0, { one: '# Error', other: '# Errors' })}`}
            </Badge>
          )}
          {(warnCount ?? 0) > 0 && (
            <Badge
              variant="outline"
              className="text-[10px] border-yellow-500/30 text-yellow-400 bg-yellow-500/10 hover:bg-yellow-500/20 transition-colors"
            >
              {t(
                i18n,
              )`${plural(warnCount ?? 0, { one: '# Warning', other: '# Warnings' })}`}
            </Badge>
          )}
          <Badge
            variant="outline"
            className="text-[10px] border-zinc-700 text-zinc-500 bg-zinc-900"
          >
            {linesTotal ? (
              t(
                i18n,
              )`${plural(linesTotal, { one: '# Line', other: '# Lines' })}`
            ) : (
              <>
                {i18n._(msg`Total`)} {totalLogs}
              </>
            )}
          </Badge>
        </div>
      </div>

      {isLoading && logs.length === 0 ? (
        <div className="flex flex-col items-center justify-center h-[500px] text-zinc-700 space-y-2 font-mono text-xs">
          <RefreshCw className="h-6 w-6 animate-spin opacity-50" />
          <p>
            <Trans>Loading logs...</Trans>
          </p>
        </div>
      ) : logs.length > 0 ? (
        <VirtualLogList logs={logs} timeFormat={timeFormat}>
          {hasNextPage && (
            <div ref={loadMoreRef} className="flex justify-center py-4">
              {isFetchingNextPage ? (
                <div className="flex items-center gap-2 text-muted-foreground text-xs">
                  <RefreshCw className="h-3 w-3 animate-spin" />
                  <Trans>Loading more logs...</Trans>
                </div>
              ) : (
                <div className="h-8" />
              )}
            </div>
          )}
        </VirtualLogList>
      ) : (
        <div className="flex flex-col items-center justify-center h-[500px] text-zinc-700 font-mono text-xs">
          <Terminal className="h-10 w-10 mb-3 opacity-20" />
          <p>
            <Trans>No logs available for this execution.</Trans>
          </p>
        </div>
      )}
    </Card>
  );
});

/**
 * Mounts the rows in fixed-size chunks and only the chunks near the viewport.
 * Rows wrap, so a chunk's height is only known once it has been on screen;
 * until then it is estimated, and the browser's scroll anchoring keeps the
 * visible rows in place when an estimate is replaced by the real height.
 */
function VirtualLogList({
  logs,
  timeFormat,
  children,
}: {
  logs: JobLogEntry[];
  timeFormat: Intl.DateTimeFormat;
  children: ReactNode;
}) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewportHeight, setViewportHeight] = useState(0);

  // Measured chunk heights by chunk index, with the row count each was
  // measured at. Held in a ref and versioned so a burst of measurements
  // re-renders once.
  const heightsRef = useRef<{ rows: number; height: number }[]>([]);
  const [, bumpHeights] = useReducer((n: number) => n + 1, 0);

  const observerRef = useRef<ResizeObserver | null>(null);
  if (observerRef.current === null && typeof ResizeObserver !== 'undefined') {
    observerRef.current = new ResizeObserver((entries) => {
      let changed = false;
      for (const entry of entries) {
        const target = entry.target as HTMLElement;
        if (target === scrollRef.current) {
          setViewportHeight(target.clientHeight);
          continue;
        }
        const index = Number(target.dataset.chunk);
        const rows = target.childElementCount;
        const height = target.offsetHeight;
        const known = heightsRef.current[index];
        if (known?.rows !== rows || known.height !== height) {
          heightsRef.current[index] = { rows, height };
          changed = true;
        }
      }
      if (changed) bumpHeights();
    });
  }
  useEffect(() => () => observerRef.current?.disconnect(), []);

  const observeViewport = useCallback((node: HTMLDivElement | null) => {
    scrollRef.current = node;
    if (!node) return;
    setViewportHeight(node.clientHeight);
    const observer = observerRef.current;
    observer?.observe(node);
    return () => observer?.unobserve(node);
  }, []);

  const observeChunk = useCallback((node: HTMLDivElement | null) => {
    if (!node) return;
    const observer = observerRef.current;
    observer?.observe(node);
    return () => observer?.unobserve(node);
  }, []);

  const chunkCount = Math.ceil(logs.length / LOG_CHUNK_SIZE);
  const chunkHeights: number[] = [];
  for (let i = 0; i < chunkCount; i++) {
    const rows = Math.min(LOG_CHUNK_SIZE, logs.length - i * LOG_CHUNK_SIZE);
    // The newest chunk grows while the job runs; a height measured before it
    // last grew no longer applies.
    const known = heightsRef.current[i];
    chunkHeights.push(
      known?.rows === rows ? known.height : rows * ESTIMATED_ROW_PX,
    );
  }
  const { first, last, before, after } = visibleLogChunks(
    chunkHeights,
    scrollTop - OVERSCAN_PX,
    scrollTop + viewportHeight + OVERSCAN_PX,
  );

  const chunks = [];
  for (let i = first; i <= last; i++) {
    chunks.push(
      <LogChunk
        key={i}
        index={i}
        logs={logs}
        start={i * LOG_CHUNK_SIZE}
        end={Math.min(logs.length, (i + 1) * LOG_CHUNK_SIZE)}
        timeFormat={timeFormat}
        observe={observeChunk}
      />,
    );
  }

  return (
    <div
      ref={observeViewport}
      onScroll={(event) => setScrollTop(event.currentTarget.scrollTop)}
      // Scrollable regions need to take focus for keyboard scrolling.
      tabIndex={0}
      className="h-[500px] w-full overflow-y-auto bg-zinc-950 [scrollbar-width:thin] [scrollbar-color:var(--color-zinc-700)_transparent] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
    >
      <div className="p-4 font-mono text-xs">
        <div style={{ height: before }} />
        {chunks}
        <div style={{ height: after }} />
        {children}
      </div>
    </div>
  );
}

interface LogChunkProps {
  index: number;
  logs: JobLogEntry[];
  start: number;
  end: number;
  timeFormat: Intl.DateTimeFormat;
  observe: (node: HTMLDivElement | null) => (() => void) | undefined;
}

/**
 * Rows `start..end` of `logs`. Re-renders only when one of its own rows
 * changed: a running job appends to the newest chunk alone, and the flattened
 * array is rebuilt on every append.
 */
const LogChunk = memo(
  function LogChunk({
    index,
    logs,
    start,
    end,
    timeFormat,
    observe,
  }: LogChunkProps) {
    const rows = [];
    for (let i = start; i < end; i++) {
      const log = logs[i];
      rows.push(
        <div
          key={i}
          className="group/log flex gap-4 hover:bg-zinc-900/50 -mx-2 px-2 py-0.5 rounded transition-colors"
        >
          <span className="text-zinc-600 shrink-0 select-none w-20 group-hover/log:text-zinc-500 transition-colors">
            {timeFormat.format(new Date(log.timestamp))}
          </span>
          <span
            className={cn(
              'break-all',
              log.level === 'ERROR'
                ? 'text-red-400 font-bold'
                : log.level === 'WARN'
                  ? 'text-yellow-400'
                  : 'text-zinc-300',
            )}
          >
            {log.message}
          </span>
        </div>,
      );
    }
    return (
      <div ref={observe} data-chunk={index} className="space-y-1 pb-1">
        {rows}
      </div>
    );
  },
  (prev, next) => {
    if (
      prev.index !== next.index ||
      prev.start !== next.start ||
      prev.end !== next.end ||
      prev.timeFormat !== next.timeFormat ||
      prev.observe !== next.observe
    ) {
      return false;
    }
    return sameLogRows(prev.logs, next.logs, next.start, next.end);
  },
);
