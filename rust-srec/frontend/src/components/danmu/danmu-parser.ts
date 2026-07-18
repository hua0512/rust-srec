import { SaxesParser, type SaxesTagPlain } from 'saxes';

export interface DanmuComment {
  time: number;
  mode: number;
  timestampMs: number | null;
  userHash: string;
  rowId: string;
  username?: string;
  content: string;
}

interface ActiveComment {
  elementName: 'd' | 'gift' | 'sc';
  comment: DanmuComment;
  captureText: boolean;
}

function parseNumber(value: string | null | undefined): number | null {
  if (value == null || value.trim() === '') return null;

  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : null;
}

export function normalizeDanmuTimestamp(
  value: string | null | undefined,
): number | null {
  const timestamp = parseNumber(value);
  if (timestamp === null) return null;

  return Math.abs(timestamp) < 100_000_000_000 ? timestamp * 1000 : timestamp;
}

function parseLegacyIdentity(value: string): {
  rowId: string;
  username?: string;
} {
  const match = value.match(/^(.*?)\s+user=(.*)$/);
  if (!match) return { rowId: value };

  return { rowId: match[1], username: match[2] };
}

function parseStandardComment(
  attributes: Record<string, string>,
): DanmuComment | null {
  const parts = attributes.p?.split(',');
  if (!parts || parts.length < 8) return null;

  const time = parseNumber(parts[0]);
  const mode = parseNumber(parts[1]);
  if (time === null || mode === null) return null;

  const legacyIdentity = parseLegacyIdentity(parts[7]);
  return {
    time,
    mode,
    timestampMs: normalizeDanmuTimestamp(parts[4]),
    userHash: parts[6],
    rowId: legacyIdentity.rowId,
    username: attributes.user || legacyIdentity.username,
    content: '',
  };
}

function parseGift(
  attributes: Record<string, string>,
  index: number,
): DanmuComment | null {
  const time = parseNumber(attributes.ts);
  if (time === null) return null;

  const giftName = attributes.giftname || 'Gift';
  const giftCount = attributes.giftcount;
  return {
    time,
    mode: 1,
    timestampMs: normalizeDanmuTimestamp(attributes.timestamp),
    userHash: attributes.uid || '',
    rowId: `gift-${index}`,
    username: attributes.user || undefined,
    content: giftCount ? `${giftName} x${giftCount}` : giftName,
  };
}

function parseSuperChat(
  attributes: Record<string, string>,
  index: number,
): DanmuComment | null {
  const time = parseNumber(attributes.ts);
  if (time === null) return null;

  return {
    time,
    mode: 5,
    timestampMs: normalizeDanmuTimestamp(attributes.timestamp),
    userHash: attributes.uid || '',
    rowId: `sc-${index}`,
    username: attributes.user || undefined,
    content: '',
  };
}

export class DanmuStreamParser {
  private readonly parser = new SaxesParser({ xmlns: false, position: false });
  private readonly comments: DanmuComment[] = [];
  private activeComment: ActiveComment | null = null;
  private elementIndex = 0;
  private isChronological = true;
  private parseError: Error | null = null;

  constructor() {
    this.parser.on('opentag', (tag) => this.handleOpenTag(tag));
    this.parser.on('text', (text) => this.appendText(text));
    this.parser.on('cdata', (text) => this.appendText(text));
    this.parser.on('closetag', (tag) => this.handleCloseTag(tag));
    this.parser.on('error', (error) => {
      this.parseError ??= error;
    });
  }

  write(chunk: string): void {
    this.parser.write(chunk);
  }

  finish(): DanmuComment[] {
    this.parser.close();
    if (this.parseError) throw new Error('Invalid danmu XML');

    if (!this.isChronological) {
      this.comments.sort((a, b) => a.time - b.time);
    }
    return this.comments;
  }

  private handleOpenTag(tag: SaxesTagPlain): void {
    let comment: DanmuComment | null = null;
    let captureText = false;

    if (tag.name === 'd') {
      comment = parseStandardComment(tag.attributes);
      captureText = true;
    } else if (tag.name === 'gift') {
      comment = parseGift(tag.attributes, this.elementIndex);
    } else if (tag.name === 'sc') {
      comment = parseSuperChat(tag.attributes, this.elementIndex);
      captureText = true;
    } else {
      return;
    }

    this.elementIndex += 1;
    if (comment) {
      this.activeComment = {
        elementName: tag.name,
        comment,
        captureText,
      };
    }
  }

  private appendText(text: string): void {
    if (this.activeComment?.captureText) {
      this.activeComment.comment.content += text;
    }
  }

  private handleCloseTag(tag: SaxesTagPlain): void {
    if (tag.name !== this.activeComment?.elementName) return;

    const comment = this.activeComment.comment;
    const previous = this.comments.at(-1);
    if (previous && previous.time > comment.time) {
      this.isChronological = false;
    }
    this.comments.push(comment);
    this.activeComment = null;
  }
}

export function parseDanmuXml(text: string): DanmuComment[] {
  const parser = new DanmuStreamParser();
  parser.write(text);
  return parser.finish();
}
