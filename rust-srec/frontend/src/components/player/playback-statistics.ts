export interface PlaybackStatistics {
  width: number | null;
  height: number | null;
  bufferSeconds: number | null;
  droppedFrames: number | null;
  totalFrames: number | null;
  liveEdgeDistanceSeconds: number | null;
}

type VideoStatisticsSource = Pick<
  HTMLVideoElement,
  | 'readyState'
  | 'currentTime'
  | 'buffered'
  | 'seekable'
  | 'videoWidth'
  | 'videoHeight'
> &
  Partial<Pick<HTMLVideoElement, 'getVideoPlaybackQuality'>>;

function nonNegative(value: number | undefined): number | null {
  return value != null && Number.isFinite(value) && value >= 0 ? value : null;
}

export function readPlaybackStatistics(
  video: VideoStatisticsSource,
  isLive: boolean,
  hlsLatencySeconds?: number,
): PlaybackStatistics {
  const ready = video.readyState >= 1;
  let bufferSeconds: number | null = ready ? 0 : null;
  // Only the range containing the playhead is playable without another seek.
  for (let index = 0; ready && index < video.buffered.length; index++) {
    if (
      video.buffered.start(index) <= video.currentTime &&
      video.currentTime <= video.buffered.end(index)
    ) {
      bufferSeconds = nonNegative(
        video.buffered.end(index) - video.currentTime,
      );
      break;
    }
  }
  let liveEdgeDistanceSeconds: number | null = null;
  if (ready && isLive) {
    liveEdgeDistanceSeconds = nonNegative(hlsLatencySeconds);
    if (liveEdgeDistanceSeconds == null && video.seekable.length > 0) {
      liveEdgeDistanceSeconds = nonNegative(
        video.seekable.end(video.seekable.length - 1) - video.currentTime,
      );
    }
  }
  const quality = ready ? video.getVideoPlaybackQuality?.() : undefined;
  return {
    width: ready && video.videoWidth > 0 ? video.videoWidth : null,
    height: ready && video.videoHeight > 0 ? video.videoHeight : null,
    bufferSeconds,
    droppedFrames: nonNegative(quality?.droppedVideoFrames),
    totalFrames: nonNegative(quality?.totalVideoFrames),
    liveEdgeDistanceSeconds,
  };
}
