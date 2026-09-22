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
