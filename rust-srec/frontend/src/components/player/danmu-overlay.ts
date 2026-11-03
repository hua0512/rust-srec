import {
  FLAG_SUPERCHAT,
  MERGE_WINDOW_SECS,
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
/**
 * Live comments arrive in batches about this far apart; each batch is spread
 * over the interval so it does not land in a single frame.
 */
const LIVE_SPREAD_SECS = 0.2;
/** Live comments waiting for their turn in the spread; more are dropped. */
const MAX_PENDING = 256;
/** Distinct live texts remembered for merging repeats. */
const MAX_RECENT_TEXTS = 2048;
const FONT_WEIGHT = 700;
const TEXT_SHADOW =
  '1px 0 1px #000, -1px 0 1px #000, 0 1px 1px #000, 0 -1px 1px #000';
const HOVER_BACKGROUND = 'rgba(0, 0, 0, 0.5)';

/** A live chat message as the server relays it. */
export interface LiveDanmuItem {
  text: string;
  /** `0xRRGGBB`, or `null` for the default white. */
  color: number | null;
  kind: 'chat' | 'superchat';
  user: string;
}

/** What the overlay needs to filter and draw one comment. */
interface Comment {
  text: string;
  /** `0xRRGGBB`. */
  color: number;
  /** `TRACK_SCROLL`, `TRACK_TOP` or `TRACK_BOTTOM`. */
  position: number;
  superchat: boolean;
  sender: string;
  /**
   * The size of the repeat group this comment opens, or 0 for a later member
   * of a group, which merging hides.
   */
  repeats: number;
}

interface ActiveComment {
  comment: Comment;
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

function trackComment(track: DanmuTrack, index: number): Comment {
  return {
    text: track.texts[index],
    color: track.colors[index],
    position: track.positions[index],
    superchat: (track.flags[index] & FLAG_SUPERCHAT) !== 0,
    sender: track.senderNames[track.senders[index]],
    repeats: track.repeats[index],
  };
}

function cssColor(color: number): string {
  return `#${color.toString(16).padStart(6, '0')}`;
}

function sameList(a: readonly string[], b: readonly string[]): boolean {
  return a.length === b.length && a.every((value, index) => value === b[index]);
}

/**
 * Draws danmu over a video: a recording's track, live comments handed to
 * `push` as they arrive, or both.
 *
 * Each comment is a pooled DOM node moved by a Web Animation, so motion runs on
 * the compositor and the per-frame script only advances a cursor through the
 * sorted track and the live queue. Text width comes from a canvas, which
 * avoids layout reads. Lanes record when they free up in media time, the clock
 * the animations follow through pauses and playback-rate changes, and
 * scrolling comments share one speed so a lane's comments can never overlap; a
 * comment with no free lane is dropped. Filters are checked only for comments
 * whose time has come, so their cost follows the chat rate rather than the
 * file size.
 *
 * Live comments are timed from the media time they arrive at, and dropped
 * while the overlay is not running (paused, buffering or hidden): chat shown
 * late is worthless, so nothing is held beyond one batch's spread.
 */
export class DanmuOverlay {
  private readonly layer: HTMLElement;
  private readonly video: HTMLVideoElement;
  private readonly track: DanmuTrack | null;
  private readonly context: CanvasRenderingContext2D | null;
  private readonly resizeObserver: ResizeObserver | null;
  /** Hover needs a real pointer; on touch screens a tap would just stop text. */
  private readonly canHover: boolean;
  private readonly pool: HTMLDivElement[] = [];
  private readonly active = new Set<ActiveComment>();
  private readonly byNode = new Map<HTMLDivElement, ActiveComment>();
  private options: DanmuPreferences = defaultDanmuPreferences();
  private blockedUsers = new Set<string>();
  /** Ring buffer of live comments, each due at its `pendingTimes` entry. */
  private readonly pending: (Comment | undefined)[] = Array.from({
    length: MAX_PENDING,
  });
  private readonly pendingTimes = new Float64Array(MAX_PENDING);
  private pendingHead = 0;
  private pendingCount = 0;
  /**
   * Live text -> media time its repeat group opened. Kept in time order so
   * expired entries are evicted from the front.
   */
  private readonly recentTexts = new Map<string, number>();
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

  constructor(
    layer: HTMLElement,
    video: HTMLVideoElement,
    track: DanmuTrack | null,
  ) {
    this.layer = layer;
    this.video = video;
    this.track = track;
    this.context = document.createElement('canvas').getContext('2d');
    this.canHover =
      typeof matchMedia === 'function' && matchMedia('(hover: hover)').matches;

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
      this.blockedUsers = new Set(next.blockedUsers);
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
      if (!this.passes(comment.comment)) this.release(comment);
      else if (!next.hoverPause && comment.hovered) this.unhover(comment);
    }
    if (previous.hoverPause !== next.hoverPause) {
      for (const { node } of this.active) this.setHoverable(node);
    }
  }

  /**
   * Schedules live comments evenly from now across one batch interval. They
   * are dropped while the overlay is not running, when filtered out, and once
   * `MAX_PENDING` are waiting.
   */
  push(items: readonly LiveDanmuItem[]): void {
    if (!this.frame || items.length === 0) return;
    const now = this.video.currentTime;
    const step = LIVE_SPREAD_SECS / items.length;
    for (let offset = 0; offset < items.length; offset++) {
      if (this.pendingCount === MAX_PENDING) return;
      const item = items[offset];
      const text = item.text.trim();
      if (text === '') continue;
      const superchat = item.kind === 'superchat';
      const comment: Comment = {
        text,
        color: item.color === null ? WHITE : item.color & WHITE,
        position: superchat ? TRACK_TOP : TRACK_SCROLL,
        superchat,
        sender: item.user,
        repeats: this.isRepeat(text, now) ? 0 : 1,
      };
      if (!this.passes(comment)) continue;
      const slot = (this.pendingHead + this.pendingCount) % MAX_PENDING;
      this.pending[slot] = comment;
      this.pendingTimes[slot] = now + offset * step;
      this.pendingCount++;
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
    ['seeking', () => this.seeking()],
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
    this.dropPending();
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

    const track = this.track;
    if (track) {
      const { times } = track;
      while (this.cursor < times.length && times[this.cursor] <= now) {
        const index = this.cursor++;
        if (now - times[index] > STALE_SECS) continue;
        const comment = trackComment(track, index);
        if (this.passes(comment)) this.spawn(comment, now);
      }
    }

    while (
      this.pendingCount > 0 &&
      this.pendingTimes[this.pendingHead] <= now
    ) {
      const comment = this.pending[this.pendingHead];
      this.pending[this.pendingHead] = undefined;
      this.pendingHead = (this.pendingHead + 1) % MAX_PENDING;
      this.pendingCount--;
      if (comment) this.spawn(comment, now);
    }
  };

  /**
   * Live players nudge `currentTime` on their own to stay near the live edge,
   * so without a track only a move back, or forward past `JUMP_SECS` (the
   * rule `tick` uses), resets the overlay.
   */
  private seeking(): void {
    const now = this.video.currentTime;
    if (this.track || now < this.lastTime || now - this.lastTime > JUMP_SECS) {
      this.seekTo(now);
    } else {
      this.lastTime = now;
    }
  }

  private seekTo(time: number): void {
    this.clear();
    // Merge-window times are media times, which no longer compare.
    this.recentTexts.clear();
    if (this.track) this.cursor = lowerBound(this.track.times, time);
    this.lastTime = time;
  }

  private dropPending(): void {
    this.pending.fill(undefined);
    this.pendingHead = 0;
    this.pendingCount = 0;
  }

  /**
   * Whether live `text` repeats one that opened a group within
   * `MERGE_WINDOW_SECS`; if not, it opens a new group.
   */
  private isRepeat(text: string, now: number): boolean {
    const recent = this.recentTexts;
    for (const [oldest, opened] of recent) {
      if (now - opened <= MERGE_WINDOW_SECS && recent.size < MAX_RECENT_TEXTS) {
        break;
      }
      recent.delete(oldest);
    }
    const opened = recent.get(text);
    if (opened !== undefined && now - opened <= MERGE_WINDOW_SECS) return true;
    // Re-inserting moves the text to the back, keeping the map in time order.
    recent.delete(text);
    recent.set(text, now);
    return false;
  }

  /** Whether the viewer's filters let `comment` show. */
  private passes(comment: Comment): boolean {
    const { hidden, merge, blockedKeywords } = this.options;
    const { position } = comment;
    if (position === TRACK_SCROLL && hidden.scroll) return false;
    if (position === TRACK_TOP && hidden.top) return false;
    if (position === TRACK_BOTTOM && hidden.bottom) return false;
    if (hidden.colored && comment.color !== WHITE) return false;
    if (hidden.superchat && comment.superchat) return false;
    if (merge && comment.repeats === 0) return false;
    if (this.blockedUsers.has(comment.sender)) return false;
    if (blockedKeywords.length > 0) {
      const text = comment.text.toLowerCase();
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

  private spawn(comment: Comment, now: number): void {
    if (this.active.size >= MAX_ACTIVE || this.width === 0) return;

    const { position, repeats } = comment;
    const text =
      this.options.merge && repeats > 1
        ? `${comment.text} ×${repeats}`
        : comment.text;
    let keyframes: Keyframe[];
    let durationSecs: number;

    if (position === TRACK_SCROLL) {
      const lane = freeLane(this.scrollLanes, now);
      if (lane < 0) return;
      // Measured only once a lane is free: in busy chat most comments find
      // none and are dropped.
      const textWidth = this.textWidth(text);
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
      const textWidth = this.textWidth(text);
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
    node.style.color = cssColor(comment.color);
    node.title = comment.sender;
    this.setHoverable(node);
    this.layer.append(node);

    const animation = node.animate(keyframes, {
      duration: durationSecs * 1000,
      easing: 'linear',
    });
    animation.playbackRate = this.video.playbackRate;
    const active: ActiveComment = {
      comment,
      node,
      animation,
      hovered: false,
    };
    animation.onfinish = () => this.release(active);
    this.active.add(active);
    this.byNode.set(node, active);
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
    this.dropPending();
    this.scrollLanes.fill(0);
    this.topLanes.fill(0);
    this.bottomLanes.fill(0);
  }
}
