import { memo, useMemo } from 'react';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Skeleton } from '@/components/ui/skeleton';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { motion, AnimatePresence } from 'motion/react';
import { formatBytes, formatDuration } from '@/lib/format';
import {
  FileVideo,
  Download,
  Play,
  Video,
  MessageSquare,
  Timer,
  Scissors,
  Link2,
  Unlink,
  ArrowRight,
} from 'lucide-react';
import { isPlayable } from '@/lib/media';
import { MediaOutput } from '@/api/schemas/system';
import type { SessionSegment } from '@/api/schemas/session';
import { formatSplitReason, SplitReasonDetails } from '@/lib/split-reason';
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from '@/components/ui/tooltip';

function isNonEmptyString(value: unknown): value is string {
  return typeof value === 'string' && value.trim().length > 0;
}

interface RecordingsTabProps {
  isLoading: boolean;
  outputs: MediaOutput[];
  segments?: SessionSegment[];
  isSegmentsLoading?: boolean;
  onDownload: (id: string, name: string) => void;
  onPlay: (output: MediaOutput) => void;
}

type SplitReasonRecord = {
  code?: string | null;
  details?: unknown;
};

/** Intermediate shape used during grouping (mutable, optional fields). */
interface TimelineGroupBuilder {
  id: string;
  baseName: string;
  startTimeMs?: number;
  endTimeMs: number;
  durationSecs: number | null;
  sizeBytes: number;
  outputs: MediaOutput[];
  splitReason?: SplitReasonRecord;
}

/** Finalized group with all fields resolved — no optional timestamps. */
interface TimelineGroup {
  id: string;
  baseName: string;
  startTimeMs: number;
  endTimeMs: number;
  durationSecs: number | null;
  sizeBytes: number;
  outputs: MediaOutput[];
  splitReason?: SplitReasonRecord;
  hasBreakBefore: boolean;
  gapSecsBefore: number;
}

const MAX_STAGGER_DELAY = 0.5;

// --- Extracted child components ---

const SplitReasonBadge = memo(function SplitReasonBadge({
  splitReason,
  isSegmentsLoading,
}: {
  splitReason?: SplitReasonRecord;
  isSegmentsLoading?: boolean;
}) {
  const { i18n } = useLingui();
  if (isSegmentsLoading || !splitReason || splitReason.code === 'discontinuity')
    return null;
  const formattedReason = formatSplitReason(i18n, splitReason);
  if (!formattedReason) return null;

  return (
    <Tooltip delayDuration={200}>
      <TooltipTrigger asChild>
        <Badge
          variant="outline"
          className="h-5 px-1.5 gap-1 text-[10px] max-w-24 sm:max-w-40 cursor-help font-normal"
        >
          <Scissors className="h-2.5 w-2.5 opacity-70 shrink-0" />
          <span className="truncate">{formattedReason}</span>
        </Badge>
      </TooltipTrigger>
      <TooltipContent
        side="bottom"
        sideOffset={6}
        className="max-w-[min(720px,calc(100vw-2rem))] px-3 py-2 bg-background text-foreground border shadow-xl z-[100]"
      >
        <div className="text-xs font-medium">
          <Trans>Split Reason</Trans>: {formattedReason}
        </div>
        <SplitReasonDetails
          code={splitReason.code ?? ''}
          details={splitReason.details}
        />
      </TooltipContent>
    </Tooltip>
  );
});

const OutputRow = memo(function OutputRow({
  output,
  onDownload,
  onPlay,
}: {
  output: MediaOutput;
  onDownload: (id: string, name: string) => void;
  onPlay: (output: MediaOutput) => void;
}) {
  const fileName = output.file_path.split('/').pop();

  return (
    <div className="p-3 px-4 flex flex-col sm:flex-row sm:items-center justify-between gap-3 hover:bg-muted/10 transition-colors">
      <div className="flex items-center gap-3 overflow-hidden min-w-0">
        <FileVideo className="h-4 w-4 text-primary/50 shrink-0" />
        <div className="flex items-center gap-2 min-w-0">
          <Badge
            variant="outline"
            className="text-[9px] px-1 h-4 uppercase shrink-0"
          >
            {output.format}
          </Badge>
          <p
            className="text-xs truncate font-mono text-foreground/80"
            title={fileName}
          >
            {fileName}
          </p>
        </div>
      </div>
      <div className="flex items-center gap-2 shrink-0">
        <Button
          variant="outline"
          size="sm"
          className="h-7 text-[10px]"
          onClick={() => onDownload(output.id, fileName || 'video')}
        >
          <Download className="mr-1.5 h-3 w-3" /> <Trans>Download</Trans>
        </Button>
        {output.format === 'DANMU_XML' && (
          <Button
            variant="secondary"
            size="sm"
            className="h-7 text-[10px]"
            onClick={(e) => {
              e.stopPropagation();
              onPlay(output);
            }}
          >
            <MessageSquare className="mr-1.5 h-3 w-3" />{' '}
            <Trans>View Danmu</Trans>
          </Button>
        )}
        {isPlayable(output) && (
          <Button
            variant="default"
            size="sm"
            className="h-7 text-[10px]"
            onClick={(e) => {
              e.stopPropagation();
              onPlay(output);
            }}
          >
            <Play className="mr-1.5 h-3 w-3" /> <Trans>Play</Trans>
          </Button>
        )}
      </div>
    </div>
  );
});

const TimelineNode = memo(function TimelineNode({
  group,
  index,
  isSegmentsLoading,
  onDownload,
  onPlay,
}: {
  group: TimelineGroup;
  index: number;
  isSegmentsLoading?: boolean;
  onDownload: (id: string, name: string) => void;
  onPlay: (output: MediaOutput) => void;
}) {
  const { i18n } = useLingui();
  const delay = Math.min(index * 0.05, MAX_STAGGER_DELAY);

  return (
    <motion.div
      initial={{ opacity: 0, x: -10 }}
      animate={{ opacity: 1, x: 0 }}
      transition={{ delay }}
      className="relative"
    >
      {/* Timeline Dot */}
      <div className="absolute -left-[31px] top-4 h-3.5 w-3.5 rounded-full bg-primary/20 border-2 border-card ring-2 ring-primary/40 z-10" />

      {/* Break or Split indicator */}
      {index > 0 && (
        <div className="absolute -top-6 -left-[38px] flex items-center gap-2 bg-card rounded-md">
          {group.hasBreakBefore ? (
            <Badge
              variant="destructive"
              className="h-5 px-1.5 gap-1 font-mono text-[10px]"
            >
              <Unlink className="h-3 w-3" />
              <Trans>Break</Trans>{' '}
              {group.gapSecsBefore > 0
                ? `(${formatDuration(group.gapSecsBefore)})`
                : ''}
            </Badge>
          ) : (
            <Badge
              variant="secondary"
              className="h-5 px-1.5 gap-1 text-[10px] text-primary bg-primary/10 border-primary/20"
            >
              <Link2 className="h-3 w-3" />
              <Trans>Lossless Split</Trans>
            </Badge>
          )}
          <SplitReasonBadge
            splitReason={group.splitReason}
            isSegmentsLoading={isSegmentsLoading}
          />
        </div>
      )}

      {/* Group Card */}
      <div className="bg-card w-full border border-border/40 hover:border-border/80 transition-colors rounded-xl overflow-hidden shadow-sm hover:shadow-md">
        <div className="bg-muted/30 px-4 py-3 border-b border-border/30 flex flex-col sm:flex-row sm:items-center justify-between gap-2">
          <div>
            <h4
              className="font-semibold text-sm truncate"
              title={group.baseName}
            >
              {group.baseName}
            </h4>
            <div className="flex items-center gap-3 text-xs text-muted-foreground mt-1">
              <span className="font-mono flex items-center gap-1">
                {i18n.date(new Date(group.startTimeMs), {
                  hour: '2-digit',
                  minute: '2-digit',
                  second: '2-digit',
                })}
                {group.startTimeMs !== group.endTimeMs && (
                  <>
                    <ArrowRight className="h-3 w-3 opacity-50" />
                    {i18n.date(new Date(group.endTimeMs), {
                      hour: '2-digit',
                      minute: '2-digit',
                      second: '2-digit',
                    })}
                  </>
                )}
              </span>
              <span>•</span>
              <span>{formatBytes(group.sizeBytes)}</span>
              {group.durationSecs && group.durationSecs > 0 && (
                <>
                  <span>•</span>
                  <span className="flex items-center gap-1">
                    <Timer className="h-3 w-3 opacity-50" />
                    {formatDuration(group.durationSecs, {
                      showSeconds: true,
                    })}
                  </span>
                </>
              )}
            </div>
          </div>
        </div>

        {/* Files within Group */}
        <div className="divide-y divide-border/20">
          {group.outputs.map((out) => (
            <OutputRow
              key={out.id}
              output={out}
              onDownload={onDownload}
              onPlay={onPlay}
            />
          ))}
        </div>
      </div>
    </motion.div>
  );
});

// --- Main component ---

export function RecordingsTab({
  isLoading,
  outputs,
  segments,
  isSegmentsLoading,
  onDownload,
  onPlay,
}: RecordingsTabProps) {
  const timelineGroups = useMemo(() => {
    interface SegmentInfo {
      splitReason: SplitReasonRecord;
      durationSecs?: number;
      startedAtMs: number;
      completedAtMs?: number;
    }
    const segmentInfoByPath = new Map<string, SegmentInfo>();
    for (const s of segments || []) {
      if (!isNonEmptyString(s.file_path) || !s.created_at) continue;
      const info: SegmentInfo = {
        splitReason: {
          code: s.split_reason_code,
          details: s.split_reason_details,
        },
        durationSecs: s.duration_secs || undefined,
        startedAtMs: new Date(s.created_at).getTime(),
        completedAtMs: s.completed_at
          ? new Date(s.completed_at).getTime()
          : undefined,
      };
      const fileName = s.file_path.split('/').pop();
      for (const key of [s.file_path, fileName]) {
        if (!isNonEmptyString(key) || segmentInfoByPath.has(key)) continue;
        segmentInfoByPath.set(key, info);
      }
    }

    const groupsMap = new Map<string, TimelineGroupBuilder>();
    for (const output of outputs) {
      const lastDot = output.file_path.lastIndexOf('.');
      const lastSlash = output.file_path.lastIndexOf('/');
      const isExtension = lastDot > lastSlash;
      const basePath = isExtension
        ? output.file_path.substring(0, lastDot)
        : output.file_path;
      const baseName = basePath.split('/').pop() || 'Unknown';

      const fileName = output.file_path.split('/').pop();
      const segInfo =
        segmentInfoByPath.get(output.file_path) ??
        (isNonEmptyString(fileName)
          ? segmentInfoByPath.get(fileName)
          : undefined);

      let group = groupsMap.get(basePath);
      if (!group) {
        group = {
          id: basePath,
          baseName,
          endTimeMs: new Date(output.created_at).getTime(),
          durationSecs: null,
          sizeBytes: 0,
          outputs: [],
        };
        groupsMap.set(basePath, group);
      } else {
        const outTs = new Date(output.created_at).getTime();
        if (outTs > group.endTimeMs) {
          group.endTimeMs = outTs;
        }
      }

      if (
        segInfo?.startedAtMs &&
        (!group.startTimeMs || segInfo.startedAtMs < group.startTimeMs)
      ) {
        group.startTimeMs = segInfo.startedAtMs;
      }
      if (segInfo?.completedAtMs) {
        group.endTimeMs = Math.max(group.endTimeMs, segInfo.completedAtMs);
      }

      const effectiveDur =
        segInfo?.durationSecs ?? output.duration_secs ?? null;
      if (
        effectiveDur &&
        (!group.durationSecs || effectiveDur > group.durationSecs)
      ) {
        group.durationSecs = effectiveDur;
      }

      group.sizeBytes += output.file_size_bytes;
      group.outputs.push(output);

      if (!group.splitReason && segInfo?.splitReason) {
        group.splitReason = segInfo.splitReason;
      }
    }

    // Finalize: resolve optional startTimeMs, compute gaps
    const builders = Array.from(groupsMap.values());
    const finalized: TimelineGroup[] = builders.map((b) => ({
      ...b,
      startTimeMs: b.startTimeMs ?? b.endTimeMs,
      hasBreakBefore: false,
      gapSecsBefore: 0,
    }));

    finalized.sort((a, b) => a.startTimeMs - b.startTimeMs);

    for (let i = 1; i < finalized.length; i++) {
      const current = finalized[i];
      const prev = finalized[i - 1];
      const gapSecs = Math.max(
        0,
        (current.startTimeMs - prev.endTimeMs) / 1000,
      );

      current.gapSecsBefore = gapSecs;
      current.hasBreakBefore =
        current.splitReason?.code === 'discontinuity' || gapSecs > 5;
    }

    return finalized;
  }, [outputs, segments]);

  return (
    <motion.div
      initial={{ opacity: 0, y: 10 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ delay: 0.2 }}
    >
      <Card className="bg-card/40 backdrop-blur-sm border-border/40 shadow-sm">
        <CardHeader className="border-b border-border/40 pb-4 flex flex-row items-center justify-between">
          <CardTitle className="text-lg font-semibold flex items-center gap-2">
            <FileVideo className="h-5 w-5 text-primary/70" />
            <Trans>Media Timeline</Trans>
          </CardTitle>
          <Badge variant="secondary" className="font-mono text-xs">
            {timelineGroups.length} <Trans>Segments</Trans> ({outputs.length}{' '}
            <Trans>Files</Trans>)
          </Badge>
        </CardHeader>
        <CardContent className="p-0">
          {isLoading ? (
            <div className="p-6 space-y-4">
              <Skeleton className="h-24 w-full rounded-xl" />
              <Skeleton className="h-24 w-full rounded-xl" />
            </div>
          ) : outputs.length === 0 ? (
            <div className="p-10 text-center text-muted-foreground">
              <Video className="h-10 w-10 mx-auto mb-3 opacity-20" />
              <p>
                <Trans>No media outputs generated yet.</Trans>
              </p>
            </div>
          ) : (
            <div className="p-4 sm:p-6">
              <div className="relative pl-6 ml-2 sm:ml-4 border-l-2 border-border/60 pb-4 space-y-8">
                <AnimatePresence mode="popLayout">
                  {timelineGroups.map((group, index) => (
                    <TimelineNode
                      key={group.id}
                      group={group}
                      index={index}
                      isSegmentsLoading={isSegmentsLoading}
                      onDownload={onDownload}
                      onPlay={onPlay}
                    />
                  ))}
                </AnimatePresence>
              </div>
            </div>
          )}
        </CardContent>
      </Card>
    </motion.div>
  );
}
