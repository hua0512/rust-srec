import { describe, expect, it, vi } from 'vitest';

import { hasSeekSettled, MpegtsPlaybackController } from '../mpegts-playback';

function bufferedRanges(...ranges: Array<[number, number]>): TimeRanges {
  return {
    length: ranges.length,
    start: (index: number) => ranges[index][0],
    end: (index: number) => ranges[index][1],
  };
}

describe('hasSeekSettled', () => {
  it('rejects seeked events without readable media data', () => {
    expect(
      hasSeekSettled(
        {
          currentTime: 223,
          readyState: 1,
          buffered: bufferedRanges(),
        },
        223,
        false,
        false,
      ),
    ).toBe(false);
  });

  it('rejects a readable frame outside the requested buffered range', () => {
    expect(
      hasSeekSettled(
        {
          currentTime: 223,
          readyState: 4,
          buffered: bufferedRanges([0, 150]),
        },
        223,
        true,
        false,
      ),
    ).toBe(false);
  });

  it('accepts a readable frame at the requested buffered range', () => {
    expect(
      hasSeekSettled(
        {
          currentTime: 223.2,
          readyState: 4,
          buffered: bufferedRanges([222.05, 369.8]),
        },
        223,
        true,
        false,
      ),
    ).toBe(true);
  });

  it('rejects buffered media until a target frame is rendered', () => {
    expect(
      hasSeekSettled(
        {
          currentTime: 223,
          readyState: 4,
          buffered: bufferedRanges([222.05, 369.8]),
        },
        223,
        false,
        false,
      ),
    ).toBe(false);
  });
});

describe('MpegtsPlaybackController', () => {
  it('uses the public currentTime setter only when rebuilding a stalled seek', () => {
    vi.useFakeTimers();
    let requestedTime = 0;
    const player = {
      on: vi.fn(),
      attachMediaElement: vi.fn(),
      load: vi.fn(),
      unload: vi.fn(),
      detachMediaElement: vi.fn(),
      destroy: vi.fn(),
      play: vi.fn(),
      get currentTime() {
        return requestedTime;
      },
      set currentTime(value: number) {
        requestedTime = value;
      },
    };
    const createPlayer = vi.fn(() => player);
    const mpegts = {
      createPlayer,
      Events: { ERROR: 'error' },
    } as unknown as ConstructorParameters<typeof MpegtsPlaybackController>[0];
    const controller = new MpegtsPlaybackController(mpegts, {
      mediaType: 'flv',
      isLive: false,
      onLoadingChange: vi.fn(),
      onError: vi.fn(),
      onStalled: vi.fn(),
      onWarning: vi.fn(),
    });

    controller.attach(
      {
        paused: false,
        currentTime: 0,
        readyState: 1,
        buffered: bufferedRanges(),
      } as HTMLVideoElement,
      '/recording.flv',
    );
    expect(createPlayer).toHaveBeenCalledWith(
      expect.anything(),
      expect.objectContaining({ accurateSeek: false }),
    );

    expect(controller.seek(223)).toBe(true);
    expect(requestedTime).toBe(0);

    vi.advanceTimersByTime(3_000);

    expect(requestedTime).toBe(223);

    controller.destroy();
    vi.useRealTimers();
  });
});
