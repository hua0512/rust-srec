/** Zero-based page indices, with gaps between the current page and the ends. */
export function getPaginationPages(
  totalPages: number,
  currentPage: number,
): (number | 'ellipsis')[] {
  const pages: (number | 'ellipsis')[] = [];
  if (totalPages <= 7) {
    for (let i = 0; i < totalPages; i++) pages.push(i);
  } else {
    pages.push(0);
    if (currentPage > 2) pages.push('ellipsis');
    for (
      let i = Math.max(1, currentPage - 1);
      i <= Math.min(totalPages - 2, currentPage + 1);
      i++
    ) {
      pages.push(i);
    }
    if (currentPage < totalPages - 3) pages.push('ellipsis');
    pages.push(totalPages - 1);
  }
  return pages;
}

/**
 * The zero-based page index to show when `pageIndex` may lie past the last
 * page, as it does once a delete or a refetch shrinks the result set. An empty
 * result set has no pages, and its only valid index is the first.
 */
export function clampPageIndex(pageIndex: number, totalPages: number): number {
  return Math.max(0, Math.min(pageIndex, totalPages - 1));
}
