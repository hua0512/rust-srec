import { useEffect, useId, useMemo } from 'react';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import {
  Pagination,
  PaginationContent,
  PaginationEllipsis,
  PaginationItem,
  PaginationLink,
  PaginationNext,
  PaginationPrevious,
} from '@/components/ui/pagination';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { clampPageIndex, getPaginationPages } from '@/lib/pagination';
import { cn } from '@/lib/utils';

export const LIST_PAGE_SIZES = [12, 24, 48, 96] as const;

interface ListPaginationProps {
  /** Zero-based index of the page on screen. */
  pageIndex: number;
  /**
   * Rows matching the current filters across every page, or `undefined` until
   * the first response. An unknown total leaves `pageIndex` alone, so a
   * deep-linked page is not clamped to the first before the data arrives.
   */
  totalCount: number | undefined;
  pageSize: number;
  /** Receives a zero-based index; the page converts it for its URL. */
  onPageIndexChange: (pageIndex: number) => void;
  /** Shows the page-size picker when given. */
  onPageSizeChange?: (pageSize: number) => void;
  pageSizes?: readonly number[];
  'aria-label'?: string;
  className?: string;
}

/**
 * The footer of a paginated list: the page-size picker and the page links.
 *
 * It renders nothing for a single page but stays mounted, because it also
 * moves the page back into range when the total shrinks under it — deleting
 * the last rows of the last page would otherwise leave an empty page with no
 * way to tell it apart from an empty list.
 */
export function ListPagination({
  pageIndex,
  totalCount,
  pageSize,
  onPageIndexChange,
  onPageSizeChange,
  pageSizes = LIST_PAGE_SIZES,
  'aria-label': ariaLabel,
  className,
}: ListPaginationProps) {
  const { i18n } = useLingui();
  const pageSizeLabelId = useId();
  const totalPages =
    totalCount === undefined ? undefined : Math.ceil(totalCount / pageSize);

  useEffect(() => {
    if (totalPages === undefined) return;
    const clamped = clampPageIndex(pageIndex, totalPages);
    if (clamped !== pageIndex) onPageIndexChange(clamped);
  }, [pageIndex, totalPages, onPageIndexChange]);

  const pages = useMemo(
    () => getPaginationPages(totalPages ?? 0, pageIndex),
    [totalPages, pageIndex],
  );

  if (totalPages === undefined || totalPages <= 1) return null;

  const isFirst = pageIndex <= 0;
  const isLast = pageIndex >= totalPages - 1;

  return (
    <div
      className={cn(
        'mt-8 flex items-center justify-between gap-4 border-t pt-6',
        className,
      )}
    >
      {onPageSizeChange ? (
        <div className="flex shrink-0 items-center gap-2">
          <span
            id={pageSizeLabelId}
            className="text-xs text-muted-foreground sm:text-sm"
          >
            <Trans>Per page:</Trans>
          </span>
          <Select
            value={pageSize.toString()}
            onValueChange={(value) => onPageSizeChange(Number(value))}
          >
            {/* No fixed width: one narrower than the trigger's padding plus
                chevron clips the value. */}
            <SelectTrigger
              aria-labelledby={pageSizeLabelId}
              className="h-8 w-auto text-xs sm:text-sm"
            >
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {pageSizes.map((size) => (
                <SelectItem key={size} value={size.toString()}>
                  {size}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
      ) : (
        <span aria-hidden="true" />
      )}

      <Pagination
        className="mx-0 w-auto min-w-0 justify-end"
        aria-label={ariaLabel}
      >
        <PaginationContent className="flex-nowrap">
          <PaginationItem>
            <PaginationPrevious
              onClick={() => onPageIndexChange(pageIndex - 1)}
              disabled={isFirst}
              className="h-8 cursor-pointer px-2 text-xs sm:px-3 sm:text-sm"
            />
          </PaginationItem>

          {pages.map((page, idx) => {
            if (page === 'ellipsis') {
              return (
                <PaginationItem
                  key={`ellipsis-${idx}`}
                  aria-hidden="true"
                  className="hidden sm:block"
                >
                  <PaginationEllipsis />
                </PaginationItem>
              );
            }
            const pageNumber = page + 1;
            return (
              <PaginationItem key={page} className="hidden sm:block">
                <PaginationLink
                  isActive={page === pageIndex}
                  onClick={() => onPageIndexChange(page)}
                  aria-label={i18n._(msg`Go to page ${pageNumber}`)}
                  className="h-8 w-8 cursor-pointer text-xs font-medium"
                >
                  {pageNumber}
                </PaginationLink>
              </PaginationItem>
            );
          })}

          <PaginationItem className="px-4 text-xs font-medium whitespace-nowrap text-muted-foreground sm:hidden">
            {pageIndex + 1} / {totalPages}
          </PaginationItem>

          <PaginationItem>
            <PaginationNext
              onClick={() => onPageIndexChange(pageIndex + 1)}
              disabled={isLast}
              className="h-8 cursor-pointer px-2 text-xs sm:px-3 sm:text-sm"
            />
          </PaginationItem>
        </PaginationContent>
      </Pagination>
    </div>
  );
}
