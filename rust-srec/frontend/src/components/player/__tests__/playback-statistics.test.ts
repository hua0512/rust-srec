import { readPlaybackStatistics } from '../playback-statistics';

function ranges(...items: [number, number][]): TimeRanges {
  return {
    length: items.length,
    start: (index) => items[index][0],
    end: (index) => items[index][1],
  };
}

function video() {
  return {
    readyState: 4,
    currentTime: 12,
    videoWidth: 1920,
    videoHeight: 1080,
    buffered: ranges([10, 16], [20, 30]),
    seekable: ranges([0, 30]),
  };
}

it('counts only buffer ahead in the current range, not across a gap', () => {
  expect(readPlaybackStatistics(video(), false)).toMatchObject({
    width: 1920,
    height: 1080,
    bufferSeconds: 4,
    liveEdgeDistanceSeconds: null,
  });
  expect(
    readPlaybackStatistics({ ...video(), currentTime: 18 }, false)
      .bufferSeconds,
  ).toBe(0);
});

it('does not confuse unknown browser frame counters with zero dropped frames', () => {
  expect(readPlaybackStatistics(video(), false).droppedFrames).toBeNull();
  const quality = {
    droppedVideoFrames: 0,
    totalVideoFrames: 100,
    corruptedVideoFrames: 0,
    creationTime: 0,
  };
  expect(
    readPlaybackStatistics(
      { ...video(), getVideoPlaybackQuality: () => quality },
      false,
    ),
  ).toMatchObject({ droppedFrames: 0, totalFrames: 100 });
});

it('prefers HLS live-edge distance and falls back to the seekable window for other engines', () => {
  expect(
    readPlaybackStatistics(video(), true, 4.5).liveEdgeDistanceSeconds,
  ).toBe(4.5);
  expect(readPlaybackStatistics(video(), true).liveEdgeDistanceSeconds).toBe(
    18,
  );
  expect(
    readPlaybackStatistics(video(), false, 4.5).liveEdgeDistanceSeconds,
  ).toBeNull();
});

it('reports unavailable values before metadata arrives and for unknown live windows', () => {
  expect(
    Object.values(readPlaybackStatistics({ ...video(), readyState: 0 }, true)),
  ).toEqual(Array(6).fill(null));
  expect(
    readPlaybackStatistics({ ...video(), seekable: ranges() }, true, Infinity)
      .liveEdgeDistanceSeconds,
  ).toBeNull();
  expect(
    readPlaybackStatistics({ ...video(), videoWidth: 0, videoHeight: 0 }, false)
      .width,
  ).toBeNull();
});
