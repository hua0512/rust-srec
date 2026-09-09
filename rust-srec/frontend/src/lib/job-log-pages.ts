import type { InfiniteData } from '@tanstack/react-query';

export type JobLogEntry = {
  timestamp: string;
  level: string;
  message: string;
};

export type JobLogPage = {
  items: JobLogEntry[];
  total: number;
  limit: number;
  offset: number;
};

/** Page params are offsets, but nothing here reads them. */
export type JobLogPages = InfiniteData<JobLogPage, unknown>;

/**
 * Offset of the only page a running job can still change. Log rows are written
 * in order and never rewritten, so every page before this one is final and a
 * live view only has to re-read this window.
 */
export function newestLogPageOffset(
  data: JobLogPages | undefined,
): number | undefined {
  return data?.pages[data.pages.length - 1]?.offset;
}

/** Rows the pages hold, in order, oldest first. */
export function flattenLogPages(data: JobLogPages | undefined): JobLogEntry[] {
  return data?.pages.flatMap((page) => page.items) ?? [];
}

/**
 * Total as of the most recent read. Taken from the newest page rather than the
 * first because that is the page a live view keeps re-reading; the first page
 * still reports whatever the count was when the reader opened the job.
 */
export function latestLogTotal(data: JobLogPages | undefined): number {
  return data?.pages[data.pages.length - 1]?.total ?? 0;
}

/**
 * Folds a freshly read newest page back into the loaded pages so the reader
 * sees the rows added since, and so the "is there more" answer comes from a
 * current total rather than the one the page was first loaded with.
 *
 * Only ever grows the page. A read that came back with fewer rows or a lower
 * total than what is already held describes an older state of the log — a
 * response that raced past a newer one — and taking it would drop rows that
 * are already on screen, permanently once the job is finished and nothing
 * re-reads.
 *
 * Returns the input unchanged — same reference, so React re-renders nothing —
 * when the page no longer lines up with what is loaded, or when it carries
 * nothing new.
 */
export function replaceNewestLogPage(
  data: JobLogPages | undefined,
  page: JobLogPage,
): JobLogPages | undefined {
  if (!data || data.pages.length === 0) return data;

  const lastIndex = data.pages.length - 1;
  const current = data.pages[lastIndex];
  // The reader paged on (or back to the start) while this read was in flight;
  // the window it describes is no longer the newest one.
  if (current.offset !== page.offset) return data;
  if (
    page.items.length <= current.items.length &&
    page.total <= current.total
  ) {
    return data;
  }

  const pages = data.pages.slice();
  pages[lastIndex] = page;
  return { ...data, pages };
}
