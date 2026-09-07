import {
  flattenLogPages,
  latestLogTotal,
  newestLogPageOffset,
  replaceNewestLogPage,
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

  it('discards a read for a window that is no longer the newest one', () => {
    const before = pages(page(0, 1000, 2000), page(1000, 1000, 2000));

    // Arrives after the reader paged forward, so it describes an earlier
    // window than the one now at the end.
    expect(replaceNewestLogPage(before, page(0, 1000, 2000))).toBe(before);
  });

  it('has nothing to fold into pages that were never loaded', () => {
    expect(replaceNewestLogPage(undefined, page(0, 3, 3))).toBeUndefined();
  });
});
