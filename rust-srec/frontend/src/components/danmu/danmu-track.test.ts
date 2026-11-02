import { DanmuStreamParser } from './danmu-parser';
import {
  TRACK_BOTTOM,
  TRACK_SCROLL,
  TRACK_TOP,
  buildDanmuTrack,
} from './danmu-track';

function trackOf(xml: string) {
  const parser = new DanmuStreamParser();
  parser.write(xml);
  return buildDanmuTrack(parser.finish());
}

describe('buildDanmuTrack', () => {
  it('maps modes and colours, and leaves out what the overlay cannot draw', () => {
    const track = trackOf(`
      <i>
        <d p="0.5,1,25,16711680,1700000000500,0,u1,1">scroll</d>
        <gift ts="0.7" giftname="Rocket" giftcount="1" user="Bob" uid="u2"></gift>
        <d p="1,4,25,65280,1700000001000,0,u3,2">bottom</d>
        <d p="1.2,7,25,16777215,1700000001200,0,u4,3">[advanced]</d>
        <d p="1.4,1,25,16777215,1700000001400,0,u5,4">   </d>
        <sc ts="2" user="Carol" uid="u6" price="30" time="60">pinned</sc>
      </i>
    `);

    expect(Array.from(track.times)).toEqual([0.5, 1, 2]);
    expect(Array.from(track.positions)).toEqual([
      TRACK_SCROLL,
      TRACK_BOTTOM,
      TRACK_TOP,
    ]);
    expect(Array.from(track.colors)).toEqual([0xff0000, 0x00ff00, 0xffffff]);
    expect(track.texts).toEqual(['scroll', 'bottom', 'pinned']);
  });

  it('groups repeats within the merge window', () => {
    const track = trackOf(`
      <i>
        <d p="0,1,25,16777215,0,0,a,1" user="Alice">gg</d>
        <d p="5,1,25,16777215,0,0,b,2" user="Bob">gg</d>
        <d p="9,1,25,16777215,0,0,a,3" user="Alice">nice</d>
        <d p="12,1,25,16777215,0,0,c,4" user="Carol">gg</d>
      </i>
    `);

    // The third "gg" is 12 s after the group's first, so it starts a new one.
    expect(Array.from(track.repeats)).toEqual([2, 0, 1, 1]);
  });
});
