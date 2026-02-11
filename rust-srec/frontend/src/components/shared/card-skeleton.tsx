import { Skeleton } from '@/components/ui/skeleton';
import { cn } from '@/lib/utils';

interface CardSkeletonProps {
  children?: React.ReactNode;
  className?: string;
}

/**
 * A reusable skeleton card for loading states in grid-based list pages.
 * Accepts optional children for custom inner layouts; otherwise renders a
 * sensible default skeleton layout.
 */
export function CardSkeleton({ children, className }: CardSkeletonProps) {
  return (
    <div
      className={cn(
        'min-h-[220px] border rounded-xl bg-muted/10 animate-pulse flex flex-col p-6 space-y-4 shadow-sm overflow-hidden',
        className,
      )}
    >
      {children ?? <CardSkeletonDefault />}
    </div>
  );
}

function CardSkeletonDefault() {
  return (
    <>
      <div className="flex justify-between items-start">
        <Skeleton className="h-10 w-10 rounded-full" />
        <Skeleton className="h-6 w-16" />
      </div>
      <div className="space-y-2 pt-2">
        <Skeleton className="h-6 w-3/4" />
        <Skeleton className="h-4 w-1/2" />
      </div>
      <div className="pt-4 mt-auto">
        <Skeleton className="h-16 w-full rounded-md" />
      </div>
    </>
  );
}
