import {
  DanmuStreamParser,
  normalizeDanmuTimestamp,
  parseDanmuXml,
} from './danmu-parser';
import { formatDanmuOffset } from './danmu-viewer';

describe('parseDanmuXml', () => {
  it('parses rust-srec timestamps and username attributes', () => {
    const [comment] = parseDanmuXml(`
      <i>
        <d p="1.500,1,25,16777215,1700000001500,0,1234,1" user="Alice">Hello</d>
      </i>
    `);

    expect(comment).toMatchObject({
      time: 1.5,
      timestampMs: 1_700_000_001_500,
      username: 'Alice',
      rowId: '1',
      content: 'Hello',
    });
  });

  it('normalizes canonical Bilibili timestamps in seconds', () => {
    const [comment] = parseDanmuXml(`
      <i>
        <d p="2,1,25,16777215,1700000001,0,abcd,2">World</d>
      </i>
    `);

    expect(comment.timestampMs).toBe(1_700_000_001_000);
  });

  it('keeps compatibility with usernames embedded in the row ID', () => {
    const [comment] = parseDanmuXml(`
      <i>
        <d p="0,1,25,16777215,1700000001,0,abcd,3 user=Legacy User">Hi</d>
      </i>
    `);

    expect(comment.rowId).toBe('3');
    expect(comment.username).toBe('Legacy User');
  });

  it('includes gift and super-chat elements in timeline order', () => {
    const comments = parseDanmuXml(`
      <i>
        <sc ts="2.500" user="Carol" uid="u2" price="30" time="60" timestamp="1700000002500">Pinned message</sc>
        <gift ts="1.500" giftname="Rocket" giftcount="5" price="100" user="Bob" uid="u1" timestamp="1700000001500"></gift>
      </i>
    `);

    expect(comments).toHaveLength(2);
    expect(comments[0]).toMatchObject({
      time: 1.5,
      mode: 1,
      username: 'Bob',
      userHash: 'u1',
      content: 'Rocket x5',
    });
    expect(comments[1]).toMatchObject({
      time: 2.5,
      mode: 5,
      username: 'Carol',
      userHash: 'u2',
      content: 'Pinned message',
    });
  });

  it('rejects malformed XML and skips malformed messages', () => {
    expect(() => parseDanmuXml('<i><d></i>')).toThrow('Invalid danmu XML');
    expect(parseDanmuXml('<i><d p="invalid" /></i>')).toEqual([]);
  });

  it('parses XML incrementally across chunk boundaries', () => {
    const parser = new DanmuStreamParser();
    parser.write('<i><d p="1,1,25,16777215,1700000001,0,u,1" user="A">Hel');
    parser.write('lo &amp; wel');
    parser.write('come</d></i>');

    expect(parser.finish()).toMatchObject([
      { username: 'A', content: 'Hello & welcome' },
    ]);
  });
});

describe('danmu timestamps', () => {
  it('returns null for missing or invalid timestamps', () => {
    expect(normalizeDanmuTimestamp(null)).toBeNull();
    expect(normalizeDanmuTimestamp('not-a-number')).toBeNull();
  });

  it('renders the segment start explicitly', () => {
    expect(formatDanmuOffset(0)).toBe('0s');
  });
});
