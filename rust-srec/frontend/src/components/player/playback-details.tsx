import type { ReactNode } from 'react';
import { Trans } from '@lingui/react/macro';
import type { PlaybackStatistics } from './playback-statistics';

export interface SourceMediaDetails {
  codec?: string;
  fps?: number;
  bitrate?: number;
}

function Row({
  label,
  hint,
  children,
}: {
  label: ReactNode;
  hint?: string;
  children: ReactNode;
}) {
  return (
    <>
      <dt className="text-white/60" title={hint}>
        {label}
      </dt>
      <dd className="text-right tabular-nums break-words">{children}</dd>
    </>
  );
}

export function PlaybackDetails({
  statistics,
  source,
  isLive,
  liveEdgeHint,
}: {
  statistics: PlaybackStatistics | null;
  source?: SourceMediaDetails;
  isLive: boolean;
  liveEdgeHint?: string;
}) {
  const unavailable = <span className="text-white/40">—</span>;
  const seconds = (value: number | null | undefined) =>
    value == null ? unavailable : <Trans>{value.toFixed(1)} s</Trans>;
  const positive = (value: number | undefined): value is number =>
    value != null && Number.isFinite(value) && value > 0;

  return (
    <div className="space-y-3 text-xs">
      {source && (
        <section className="space-y-1.5">
          <h4 className="text-[10px] font-medium uppercase tracking-wider text-white/50">
            <Trans>Reported by source</Trans>
          </h4>
          <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1">
            <Row label={<Trans>Codec</Trans>}>
              {source.codec || unavailable}
            </Row>
            <Row label={<Trans>Frame rate</Trans>}>
              {positive(source.fps) ? (
                <Trans>{source.fps} fps</Trans>
              ) : (
                unavailable
              )}
            </Row>
            <Row label={<Trans>Bitrate</Trans>}>
              {positive(source.bitrate) ? (
                <Trans>{(source.bitrate / 1000).toFixed(0)} kbps</Trans>
              ) : (
                unavailable
              )}
            </Row>
          </dl>
        </section>
      )}
      <section className="space-y-1.5">
        <h4 className="text-[10px] font-medium uppercase tracking-wider text-white/50">
          <Trans>Measured by player</Trans>
        </h4>
        <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1">
          <Row label={<Trans>Resolution</Trans>}>
            {statistics?.width && statistics.height
              ? `${statistics.width} × ${statistics.height}`
              : unavailable}
          </Row>
          <Row label={<Trans>Buffered ahead</Trans>}>
            {seconds(statistics?.bufferSeconds)}
          </Row>
          <Row label={<Trans>Dropped frames</Trans>}>
            {statistics?.droppedFrames != null && statistics.totalFrames != null
              ? `${statistics.droppedFrames} / ${statistics.totalFrames}`
              : unavailable}
          </Row>
          {isLive && (
            <Row label={<Trans>Behind live edge</Trans>} hint={liveEdgeHint}>
              {seconds(statistics?.liveEdgeDistanceSeconds)}
            </Row>
          )}
        </dl>
      </section>
    </div>
  );
}
