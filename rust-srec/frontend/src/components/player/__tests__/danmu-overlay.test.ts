import { buildDanmuTrack } from '@/components/danmu/danmu-track';
import type { DanmuComment } from '@/components/danmu/danmu-parser';
import { DanmuOverlay } from '../danmu-overlay';
import {
  defaultDanmuPreferences,
  type DanmuPreferences,
} from '../danmu-preferences';

function configure(overlay: DanmuOverlay, patch: Partial<DanmuPreferences>) {
  overlay.configure({ ...defaultDanmuPreferences(), ...patch });
}

function comment(time: number, content: string): DanmuComment {
  return {
    kind: 'chat',
    time,
    mode: 1,
    color: null,
    timestampMs: null,
    userHash: '',
    rowId: '',
    content,
  };
}

class FakeVideo extends EventTarget {
  currentTime = 0;
  paused = false;
  playbackRate = 1;
}

let frames: FrameRequestCallback[] = [];
const animate = vi.fn<(keyframes: Keyframe[]) => Animation>(
  () =>
    ({
      playbackRate: 1,
      onfinish: null,
      play: vi.fn(),
      pause: vi.fn(),
      cancel: vi.fn(),
    }) as unknown as Animation,
);

/** Runs one animation frame at the given media time. */
function frameAt(video: FakeVideo, time: number) {
  video.currentTime = time;
  const pending = frames;
  frames = [];
  for (const callback of pending) callback(0);
}

function shown(layer: HTMLElement): string[] {
  return Array.from(layer.children, (node) => node.textContent ?? '');
}

function mount(comments: DanmuComment[]) {
  const layer = document.createElement('div');
  // 100 px tall: 16 px text on 21 px lines gives three scrolling lanes.
  Object.defineProperty(layer, 'clientWidth', { value: 800 });
  Object.defineProperty(layer, 'clientHeight', { value: 100 });
  const video = new FakeVideo();
  const overlay = new DanmuOverlay(
    layer,
    video as unknown as HTMLVideoElement,
    buildDanmuTrack(comments),
  );
  return { layer, video, overlay };
}

beforeEach(() => {
  frames = [];
  vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => {
    frames.push(callback);
    return frames.length;
  });
  vi.stubGlobal('cancelAnimationFrame', () => {
    frames = [];
  });
  vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockReturnValue(null);
  animate.mockClear();
  HTMLElement.prototype.animate = animate;
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe('DanmuOverlay', () => {
  it('drops comments that find every lane taken instead of overlapping', () => {
    const { layer, video, overlay } = mount(
      ['a', 'b', 'c', 'd', 'e'].map((text) => comment(1, text)),
    );

    frameAt(video, 1);

    expect(shown(layer)).toEqual(['a', 'b', 'c']);
    overlay.destroy();
  });

  it('keeps every comment inside the chosen area', () => {
    const { layer, video, overlay } = mount([
      ...['a', 'b', 'c'].map((text) => comment(1, text)),
      { ...comment(1, 'bottom'), mode: 4 },
    ]);
    // A quarter of 100 px holds one 21 px row.
    configure(overlay, { area: 0.25 });

    frameAt(video, 1);

    expect(shown(layer)).toEqual(['a', 'bottom']);
    const keyframes = animate.mock.calls.map(([frames]) => frames[0].transform);
    // The bottom comment sits on the area's lower edge: 25 - 21 = 4 px.
    expect(keyframes[1]).toMatch(/, 4px, 0\)$/);
    overlay.destroy();
  });

  it('hides blocked words and senders, and merges repeats', () => {
    const { layer, video, overlay } = mount([
      { ...comment(1, 'hello'), username: 'Alice' },
      { ...comment(1, 'Buy SPAM here'), username: 'Bob' },
      { ...comment(1, 'hi'), username: 'Carol' },
      { ...comment(1, 'hello'), username: 'Dan' },
    ]);
    configure(overlay, {
      merge: true,
      blockedKeywords: ['spam'],
      blockedUsers: ['Carol'],
    });

    frameAt(video, 1);

    expect(shown(layer)).toEqual(['hello ×2']);
    overlay.destroy();
  });

  it('restarts from the seek target without replaying what was skipped', () => {
    const { layer, video, overlay } = mount([
      comment(1, 'first'),
      comment(2, 'skipped'),
      comment(30, 'after seek'),
    ]);
    frameAt(video, 1);
    expect(shown(layer)).toEqual(['first']);

    video.currentTime = 29.5;
    video.dispatchEvent(new Event('seeking'));
    expect(shown(layer)).toEqual([]);

    frameAt(video, 29.5);
    frameAt(video, 30);
    expect(shown(layer)).toEqual(['after seek']);
    overlay.destroy();
  });
});
