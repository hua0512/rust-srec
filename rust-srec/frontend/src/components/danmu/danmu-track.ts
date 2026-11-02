import type { DanmuComment } from './danmu-parser';

export const TRACK_SCROLL = 0;
export const TRACK_TOP = 1;
export const TRACK_BOTTOM = 2;

/** `flags` bit marking a super chat. */
export const FLAG_SUPERCHAT = 1;

export const WHITE = 0xffffff;

/**
 * Comments with the same text this many seconds apart or less, counted from
 * the first of them, form one repeat group.
 */
export const MERGE_WINDOW_SECS = 10;

/**
 * Danmu prepared for the video overlay, stored column-wise so a recording with
 * hundreds of thousands of comments costs a few typed arrays rather than an
 * object per comment, and the numeric columns can be transferred out of the
 * parsing worker without copying.
 */
export interface DanmuTrack {
  /** Seconds from the start of the video, ascending. */
  times: Float64Array<ArrayBuffer>;
  /** `TRACK_SCROLL`, `TRACK_TOP` or `TRACK_BOTTOM`. */
  positions: Uint8Array<ArrayBuffer>;
  /** `0xRRGGBB`. */
  colors: Uint32Array<ArrayBuffer>;
  /** `FLAG_*` bits. */
  flags: Uint8Array<ArrayBuffer>;
  /** Index into `senderNames`. */
  senders: Uint32Array<ArrayBuffer>;
  /** Distinct sender names; `''` when the file names no sender. */
  senderNames: string[];
  /**
   * The size of the repeat group a comment starts, or 0 for a later member of
   * a group, which merging hides behind the first.
   */
  repeats: Uint16Array<ArrayBuffer>;
  texts: string[];
}

/**
 * Where a comment is drawn, or `null` for elements the overlay leaves out:
 * gifts, and Bilibili's advanced (7) and scripted (8) modes, which need a
 * renderer of their own.
 */
function overlayPosition(comment: DanmuComment): number | null {
  if (comment.kind === 'gift') return null;
  if (comment.kind === 'superchat') return TRACK_TOP;
  switch (comment.mode) {
    case 1:
    case 2:
    case 3:
    case 6:
      return TRACK_SCROLL;
    case 4:
      return TRACK_BOTTOM;
    case 5:
      return TRACK_TOP;
    default:
      return null;
  }
}

/** Builds the overlay track from comments in timeline order. */
export function buildDanmuTrack(comments: readonly DanmuComment[]): DanmuTrack {
  const positions: number[] = [];
  const kept: DanmuComment[] = [];
  for (const comment of comments) {
    const position = overlayPosition(comment);
    if (position === null || comment.content.trim() === '') continue;
    positions.push(position);
    kept.push(comment);
  }

  const count = kept.length;
  const track: DanmuTrack = {
    times: new Float64Array(count),
    positions: Uint8Array.from(positions),
    colors: new Uint32Array(count),
    flags: new Uint8Array(count),
    senders: new Uint32Array(count),
    senderNames: [],
    repeats: new Uint16Array(count),
    texts: kept.map((comment) => comment.content.trim()),
  };

  const senderIndexes = new Map<string, number>();
  // Text -> index of the comment that opened its current repeat group.
  const groups = new Map<string, number>();
  for (let index = 0; index < count; index++) {
    const comment = kept[index];
    const time = Math.max(0, comment.time);
    track.times[index] = time;
    track.colors[index] =
      comment.color === null ? WHITE : comment.color & WHITE;
    if (comment.kind === 'superchat') track.flags[index] |= FLAG_SUPERCHAT;

    const name = comment.username ?? '';
    let sender = senderIndexes.get(name);
    if (sender === undefined) {
      sender = track.senderNames.length;
      senderIndexes.set(name, sender);
      track.senderNames.push(name);
    }
    track.senders[index] = sender;

    const text = track.texts[index];
    const first = groups.get(text);
    if (
      first !== undefined &&
      time - track.times[first] <= MERGE_WINDOW_SECS &&
      track.repeats[first] < 0xffff
    ) {
      track.repeats[first] += 1;
    } else {
      groups.set(text, index);
      track.repeats[index] = 1;
    }
  }
  return track;
}

export function danmuTrackTransferables(track: DanmuTrack): ArrayBuffer[] {
  return [
    track.times.buffer,
    track.positions.buffer,
    track.colors.buffer,
    track.flags.buffer,
    track.senders.buffer,
    track.repeats.buffer,
  ];
}
