import {
  flattenLogPages,
  latestLogTotal,
  newestLogPageOffset,
  replaceNewestLogPage,
  visibleLogChunks,
  type JobLogPage,
  type JobLogPages,
} from './job-log-pages';

function page(offset: number, count: number, total: number): JobLogPage {
  return {
    offset,
    limit: 1000,
    total,
    items: Array.from({ length: count }, (_, i) => ({
      timestamp: new Date(offset + i).toISOString(),
      level: 'INFO',
      message: `line ${offset + i}`,
    })),
  };
}

function pages(...list: JobLogPage[]): JobLogPages {
  return { pages: list, pageParams: list.map((p) => p.offset) };
}

describe('job log pages', () => {
  it('points a live view at the only page that can still grow', () => {
    expect(newestLogPageOffset(undefined)).toBeUndefined();
    expect(
      newestLogPageOffset(pages(page(0, 1000, 2400), page(1000, 1000, 2400))),
    ).toBe(1000);
  });

  it('reads the total from the most recently refreshed page', () => {
    expect(latestLogTotal(undefined)).toBe(0);
    expect(
      latestLogTotal(pages(page(0, 1000, 1000), page(1000, 40, 1040))),
    ).toBe(1040);
  });

  it('shows rows appended to the newest page', () => {
    const before = pages(page(0, 1000, 1005), page(1000, 5, 1005));

    const after = replaceNewestLogPage(before, page(1000, 12, 1012));

    expect(flattenLogPages(after)).toHaveLength(1012);
    expect(latestLogTotal(after)).toBe(1012);
    // Pages the reader already scrolled through are left untouched.
    expect(after?.pages[0]).toBe(before.pages[0]);
  });

  it('leaves the pages alone when nothing was appended', () => {
    const before = pages(page(0, 40, 40));

    expect(replaceNewestLogPage(before, page(0, 40, 40))).toBe(before);
  });

  it('keeps the fuller page when a stale read lands after a newer one', () => {
    const before = pages(page(0, 1000, 1012), page(1000, 12, 1012));

    // A tail read that was in flight while a later one landed; taking it would
    // drop rows the reader can already see.
    expect(replaceNewestLogPage(before, page(1000, 5, 1005))).toBe(before);
  });

  it('discards a read for a window that is no longer the newest one', () => {
    const before = pages(page(0, 1000, 2000), page(1000, 1000, 2000));

    // Arrives after the reader paged forward, so it describes an earlier
    // window than the one now at the end.
    expect(replaceNewestLogPage(before, page(0, 1000, 2000))).toBe(before);
  });

  it('has nothing to fold into pages that were never loaded', () => {
    expect(replaceNewestLogPage(undefined, page(0, 3, 3))).toBeUndefined();
  });

  it('mounts the chunks a scroll window overlaps and keeps the rest as space', () => {
    const heights = [100, 300, 100, 100, 100];

    // Every chunk is either mounted or counted in a spacer, so the scrolled
    // content keeps its full height and the rows land where they belong.
    const cases: [number, number, [number, number, number, number]][] = [
      [-50, 50, [0, 0, 0, 600]],
      [150, 450, [1, 2, 100, 200]],
      // A window starting exactly where a chunk ends leaves that chunk out.
      [400, 450, [2, 2, 400, 200]],
      // Scrolled past the end, as after the newest chunk shrank back to its
      // measured height: the last chunk stays mounted.
      [900, 1000, [4, 4, 600, 0]],
    ];
    for (const [top, bottom, [first, last, before, after]] of cases) {
      expect(visibleLogChunks(heights, top, bottom)).toEqual({
        first,
        last,
        before,
        after,
      });
    }
  });
});
