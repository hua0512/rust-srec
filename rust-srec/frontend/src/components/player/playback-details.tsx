import { Trans } from '@lingui/react/macro';
import type { PlaybackStatistics } from './playback-statistics';

export interface SourceMediaDetails {
  codec?: string;
  fps?: number;
  bitrate?: number;
}

export function PlaybackDetails({
  statistics,
  source,
  isLive,
}: {
  statistics: PlaybackStatistics | null;
  source?: SourceMediaDetails;
  isLive: boolean;
}) {
  const unavailable = <Trans>Unavailable</Trans>;
  const seconds = (value: number | null | undefined) =>
    value == null ? unavailable : <Trans>{value.toFixed(1)} s</Trans>;
  return (
    <div className="space-y-4 text-sm">
      <h3 className="font-semibold">
        <Trans>Playback details</Trans>
      </h3>
      {source && (
        <section className="space-y-2">
          <p className="text-xs font-medium text-muted-foreground">
            <Trans>Reported by source</Trans>
          </p>
          <dl className="grid grid-cols-2 gap-x-3 gap-y-2">
            <dt>
              <Trans>Codec</Trans>
            </dt>
            <dd className="text-right break-words">
              {source.codec || unavailable}
            </dd>
            <dt>
              <Trans>Frame rate</Trans>
            </dt>
            <dd className="text-right tabular-nums">
              {source.fps && Number.isFinite(source.fps) && source.fps > 0 ? (
                <Trans>{source.fps} fps</Trans>
              ) : (
                unavailable
              )}
            </dd>
            <dt>
              <Trans>Bitrate</Trans>
            </dt>
            <dd className="text-right tabular-nums">
              {source.bitrate &&
              Number.isFinite(source.bitrate) &&
              source.bitrate > 0 ? (
                <Trans>{(source.bitrate / 1000).toFixed(0)} kbps</Trans>
              ) : (
                unavailable
              )}
            </dd>
          </dl>
        </section>
      )}
      <section className="space-y-2">
        <p className="text-xs font-medium text-muted-foreground">
          <Trans>Measured by player</Trans>
        </p>
        <dl className="grid grid-cols-2 gap-x-3 gap-y-2">
          <dt>
            <Trans>Resolution</Trans>
          </dt>
          <dd className="text-right tabular-nums">
            {statistics?.width && statistics.height
              ? `${statistics.width} × ${statistics.height}`
              : unavailable}
          </dd>
          <dt>
            <Trans>Buffered ahead</Trans>
          </dt>
          <dd className="text-right tabular-nums">
            {seconds(statistics?.bufferSeconds)}
          </dd>
          <dt>
            <Trans>Dropped frames</Trans>
          </dt>
          <dd className="text-right tabular-nums">
            {statistics?.droppedFrames != null && statistics.totalFrames != null
              ? `${statistics.droppedFrames} / ${statistics.totalFrames}`
              : unavailable}
          </dd>
          {isLive && (
            <>
              <dt>
                <Trans>Behind live edge</Trans>
              </dt>
              <dd className="text-right tabular-nums">
                {seconds(statistics?.liveEdgeDistanceSeconds)}
              </dd>
            </>
          )}
        </dl>
      </section>
      <p className="text-xs text-muted-foreground">
        <Trans>
          Updates while this panel is open. Some browsers and sources do not
          report every value.
        </Trans>
      </p>
      {isLive && (
        <p className="text-xs text-muted-foreground">
          <Trans>
            Live-edge distance measures how far playback trails the available
            stream, not the total broadcast delay.
          </Trans>
        </p>
      )}
    </div>
  );
}
