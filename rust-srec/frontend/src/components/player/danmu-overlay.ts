import {
  FLAG_SUPERCHAT,
  TRACK_BOTTOM,
  TRACK_SCROLL,
  TRACK_TOP,
  WHITE,
  type DanmuTrack,
} from '@/components/danmu/danmu-track';
import {
  DANMU_SPEED_SECS,
  defaultDanmuPreferences,
  type DanmuPreferences,
} from './danmu-preferences';

/** Seconds a top or bottom comment stays up. */
const FIXED_SECS = 4;
/** Share of the player height each of the top and bottom stacks may fill. */
const FIXED_AREA = 0.4;
/** Upper bound on comments on screen; more are dropped rather than queued. */
const MAX_ACTIVE = 160;
/** A comment whose time passed longer ago than this is skipped, not shown late. */
const STALE_SECS = 1;
/**
 * A larger move of `currentTime` between frames is a seek, or a background tab
 * whose animation frames were throttled; either way the overlay restarts from
 * the new position instead of replaying everything in between.
 */
const JUMP_SECS = 1.5;
const FONT_WEIGHT = 700;
const TEXT_SHADOW =
  '1px 0 1px #000, -1px 0 1px #000, 0 1px 1px #000, 0 -1px 1px #000';
const HOVER_BACKGROUND = 'rgba(0, 0, 0, 0.5)';

interface ActiveComment {
  index: number;
  node: HTMLDivElement;
  animation: Animation;
  hovered: boolean;
}

/** First index whose time is at or after `time`. */
function lowerBound(times: Float64Array, time: number): number {
  let low = 0;
  let high = times.length;
  while (low < high) {
    const middle = (low + high) >>> 1;
    if (times[middle] < time) low = middle + 1;
    else high = middle;
  }
  return low;
}

/** First lane free at `now`, or -1. Lanes hold the media time they free up. */
function freeLane(lanes: Float64Array, now: number): number {
  for (let lane = 0; lane < lanes.length; lane++) {
    if (lanes[lane] <= now) return lane;
  }
  return -1;
}

function cssColor(color: number): string {
  return `#${color.toString(16).padStart(6, '0')}`;
}

function sameList(a: readonly string[], b: readonly string[]): boolean {
  return a.length === b.length && a.every((value, index) => value === b[index]);
}

/**
 * Draws a recording's danmu over its video.
 *
 * Each comment is a pooled DOM node moved by a Web Animation, so motion runs on
 * the compositor and the per-frame script only advances a cursor through the
 * sorted track. Text width comes from a canvas, which avoids layout reads.
 * Lanes record when they free up in media time, the clock the animations
 * follow through pauses and playback-rate changes, and scrolling comments
 * share one speed so a lane's comments can never overlap; a comment with no
 * free lane is dropped. Filters are checked only for comments whose time has
 * come, so their cost follows the chat rate rather than the file size.
 */
export class DanmuOverlay {
  private readonly layer: HTMLElement;
  private readonly video: HTMLVideoElement;
  private readonly track: DanmuTrack;
  private readonly context: CanvasRenderingContext2D | null;
  private readonly resizeObserver: ResizeObserver | null;
  /** Hover needs a real pointer; on touch screens a tap would just stop text. */
  private readonly canHover: boolean;
  private readonly pool: HTMLDivElement[] = [];
  private readonly active = new Set<ActiveComment>();
  private readonly byNode = new Map<HTMLDivElement, ActiveComment>();
  private options: DanmuPreferences = defaultDanmuPreferences();
  /** Per entry of `track.senderNames`, 1 when that sender is blocked. */
  private blockedSenders: Uint8Array;
  private scrollLanes = new Float64Array(0);
  private topLanes = new Float64Array(0);
  private bottomLanes = new Float64Array(0);
  private width = 0;
  private height = 0;
  private fontSize = 0;
  private lineHeight = 0;
  private cursor = 0;
  private lastTime = 0;
  private frame = 0;

  constructor(layer: HTMLElement, video: HTMLVideoElement, track: DanmuTrack) {
    this.layer = layer;
    this.video = video;
    this.track = track;
    this.context = document.createElement('canvas').getContext('2d');
    this.canHover =
      typeof matchMedia === 'function' && matchMedia('(hover: hover)').matches;
    this.blockedSenders = new Uint8Array(track.senderNames.length);

    Object.assign(layer.style, {
      position: 'absolute',
      inset: '0',
      overflow: 'hidden',
      pointerEvents: 'none',
      contain: 'strict',
      fontWeight: String(FONT_WEIGHT),
      color: '#fff',
      textShadow: TEXT_SHADOW,
      userSelect: 'none',
    });

    for (const [event, handler] of this.mediaListeners) {
      video.addEventListener(event, handler);
    }
    this.resizeObserver =
      typeof ResizeObserver === 'undefined'
        ? null
        : new ResizeObserver(() => this.layout(false));
    this.resizeObserver?.observe(layer);
    this.layout(true);
    this.seekTo(video.currentTime);
    if (!video.paused) this.start();
  }

  /** Applies viewer preferences; comments already up follow where they can. */
  configure(next: DanmuPreferences): void {
    const previous = this.options;
    this.options = next;
    this.layer.style.opacity = String(next.opacity);

    if (!sameList(previous.blockedUsers, next.blockedUsers)) {
      const blocked = new Set(next.blockedUsers);
      this.blockedSenders = Uint8Array.from(this.track.senderNames, (name) =>
        blocked.has(name) ? 1 : 0,
      );
    }

    const relayout =
      previous.area !== next.area ||
      previous.fontScale !== next.fontScale ||
      previous.speed !== next.speed;
    // Rows, sizes and speeds of comments on screen no longer match.
    if (relayout) this.layout(true);

    if (previous.visible !== next.visible) {
      this.layer.style.display = next.visible ? '' : 'none';
      if (!next.visible) {
        this.stop();
        this.clear();
        return;
      }
      this.seekTo(this.video.currentTime);
      if (!this.video.paused) this.start();
      return;
    }
    if (relayout) return;

    for (const comment of this.active) {
      if (!this.passes(comment.index)) this.release(comment);
      else if (!next.hoverPause && comment.hovered) this.unhover(comment);
    }
    if (previous.hoverPause !== next.hoverPause) {
      for (const { node } of this.active) this.setHoverable(node);
    }
  }

  destroy(): void {
    this.stop();
    for (const [event, handler] of this.mediaListeners) {
      this.video.removeEventListener(event, handler);
    }
    this.resizeObserver?.disconnect();
    this.clear();
    this.pool.length = 0;
  }

  private readonly mediaListeners: [string, () => void][] = [
    ['playing', () => this.resume()],
    ['pause', () => this.pause()],
    ['waiting', () => this.pause()],
    ['seeking', () => this.seekTo(this.video.currentTime)],
    [
      'ratechange',
      () => {
        for (const { animation } of this.active) {
          animation.playbackRate = this.video.playbackRate;
        }
      },
    ],
  ];

  private resume(): void {
    for (const comment of this.active) {
      if (!comment.hovered) comment.animation.play();
    }
    this.start();
  }

  private pause(): void {
    this.stop();
    for (const { animation } of this.active) animation.pause();
  }

  private start(): void {
    if (this.frame || !this.options.visible) return;
    this.frame = requestAnimationFrame(this.tick);
  }

  private stop(): void {
    if (!this.frame) return;
    cancelAnimationFrame(this.frame);
    this.frame = 0;
  }

  private readonly tick = (): void => {
    this.frame = requestAnimationFrame(this.tick);
    const now = this.video.currentTime;
    if (now < this.lastTime || now - this.lastTime > JUMP_SECS) {
      this.seekTo(now);
    }
    this.lastTime = now;

    const { times } = this.track;
    while (this.cursor < times.length && times[this.cursor] <= now) {
      const index = this.cursor++;
      if (now - times[index] <= STALE_SECS && this.passes(index)) {
        this.spawn(index, now);
      }
    }
  };

  private seekTo(time: number): void {
    this.clear();
    this.cursor = lowerBound(this.track.times, time);
    this.lastTime = time;
  }

  /** Whether the viewer's filters let comment `index` show. */
  private passes(index: number): boolean {
    const { hidden, merge, blockedKeywords } = this.options;
    const track = this.track;
    const position = track.positions[index];
    if (position === TRACK_SCROLL && hidden.scroll) return false;
    if (position === TRACK_TOP && hidden.top) return false;
    if (position === TRACK_BOTTOM && hidden.bottom) return false;
    if (hidden.colored && track.colors[index] !== WHITE) return false;
    if (hidden.superchat && track.flags[index] & FLAG_SUPERCHAT) return false;
    if (merge && track.repeats[index] === 0) return false;
    if (this.blockedSenders[track.senders[index]]) return false;
    if (blockedKeywords.length > 0) {
      const text = track.texts[index].toLowerCase();
      if (blockedKeywords.some((keyword) => text.includes(keyword))) {
        return false;
      }
    }
    return true;
  }

  /**
   * Re-reads the player size; `force` also re-applies font size and rows after
   * a preference change.
   */
  private layout(force: boolean): void {
    const width = this.layer.clientWidth;
    const height = this.layer.clientHeight;
    if (!force && width === this.width && height === this.height) return;

    // Positions already on screen were computed for the old layout.
    this.clear();
    this.width = width;
    this.height = height;
    const base = Math.min(36, Math.max(16, height * 0.04));
    this.fontSize = Math.max(10, Math.round(base * this.options.fontScale));
    this.lineHeight = Math.round(this.fontSize * 1.3);
    this.layer.style.fontSize = `${this.fontSize}px`;
    this.layer.style.lineHeight = `${this.lineHeight}px`;
    if (this.context) {
      const family = getComputedStyle(this.layer).fontFamily || 'sans-serif';
      this.context.font = `${FONT_WEIGHT} ${this.fontSize}px ${family}`;
    }

    const lanes = (pixels: number) =>
      height > 0
        ? new Float64Array(Math.max(1, Math.floor(pixels / this.lineHeight)))
        : new Float64Array(0);
    const areaHeight = height * this.options.area;
    // Each stack gets at most half the area so top and bottom comments never
    // meet when the area is small.
    const fixedHeight = Math.min(areaHeight / 2, height * FIXED_AREA);
    this.scrollLanes = lanes(areaHeight);
    this.topLanes = lanes(fixedHeight);
    this.bottomLanes = lanes(fixedHeight);
  }

  private textWidth(text: string): number {
    return this.context
      ? this.context.measureText(text).width
      : text.length * this.fontSize;
  }

  private spawn(index: number, now: number): void {
    if (this.active.size >= MAX_ACTIVE || this.width === 0) return;

    const repeats = this.track.repeats[index];
    const text =
      this.options.merge && repeats > 1
        ? `${this.track.texts[index]} ×${repeats}`
        : this.track.texts[index];
    const position = this.track.positions[index];
    const textWidth = this.textWidth(text);
    let keyframes: Keyframe[];
    let durationSecs: number;

    if (position === TRACK_SCROLL) {
      const lane = freeLane(this.scrollLanes, now);
      if (lane < 0) return;
      const speed = this.width / DANMU_SPEED_SECS[this.options.speed];
      // The lane takes its next comment once this one's tail, plus a gap of
      // one line height, has entered the player.
      this.scrollLanes[lane] = now + (textWidth + this.lineHeight) / speed;
      const y = lane * this.lineHeight;
      durationSecs = (this.width + textWidth) / speed;
      keyframes = [
        { transform: `translate3d(${this.width}px, ${y}px, 0)` },
        { transform: `translate3d(${-textWidth}px, ${y}px, 0)` },
      ];
    } else {
      const lanes = position === TRACK_TOP ? this.topLanes : this.bottomLanes;
      const lane = freeLane(lanes, now);
      if (lane < 0) return;
      lanes[lane] = now + FIXED_SECS;
      const y =
        position === TRACK_TOP
          ? lane * this.lineHeight
          : Math.max(
              0,
              Math.floor(this.height * this.options.area) -
                (lane + 1) * this.lineHeight,
            );
      const x = Math.max(0, (this.width - textWidth) / 2);
      const transform = `translate3d(${x}px, ${y}px, 0)`;
      durationSecs = FIXED_SECS;
      keyframes = [{ transform }, { transform }];
    }

    const node = this.pool.pop() ?? this.createNode();
    node.textContent = text;
    node.style.color = cssColor(this.track.colors[index]);
    node.title = this.track.senderNames[this.track.senders[index]];
    this.setHoverable(node);
    this.layer.append(node);

    const animation = node.animate(keyframes, {
      duration: durationSecs * 1000,
      easing: 'linear',
    });
    animation.playbackRate = this.video.playbackRate;
    const comment: ActiveComment = {
      index,
      node,
      animation,
      hovered: false,
    };
    animation.onfinish = () => this.release(comment);
    this.active.add(comment);
    this.byNode.set(node, comment);
  }

  private setHoverable(node: HTMLDivElement): void {
    node.style.pointerEvents =
      this.canHover && this.options.hoverPause ? 'auto' : 'none';
  }

  private createNode(): HTMLDivElement {
    const node = document.createElement('div');
    Object.assign(node.style, {
      position: 'absolute',
      left: '0',
      top: '0',
      whiteSpace: 'pre',
      willChange: 'transform',
      borderRadius: '4px',
      padding: '0 4px',
      cursor: 'default',
    });
    node.addEventListener('pointerenter', () => {
      const comment = this.byNode.get(node);
      if (!comment || !this.options.hoverPause) return;
      comment.hovered = true;
      comment.animation.pause();
      node.style.background = HOVER_BACKGROUND;
    });
    node.addEventListener('pointerleave', () => {
      const comment = this.byNode.get(node);
      if (comment?.hovered) this.unhover(comment);
    });
    return node;
  }

  private unhover(comment: ActiveComment): void {
    comment.hovered = false;
    comment.node.style.background = '';
    if (!this.video.paused) comment.animation.play();
  }

  private release(comment: ActiveComment): void {
    this.active.delete(comment);
    this.byNode.delete(comment.node);
    comment.animation.onfinish = null;
    comment.animation.cancel();
    comment.node.style.background = '';
    comment.node.remove();
    this.pool.push(comment.node);
  }

  private clear(): void {
    for (const comment of this.active) this.release(comment);
    this.scrollLanes.fill(0);
    this.topLanes.fill(0);
    this.bottomLanes.fill(0);
  }
}
