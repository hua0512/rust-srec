import { Skeleton } from '@/components/ui/skeleton';

/**
 * Placeholder for the streamer edit page.
 *
 * Mirrors the real layout — `max-w-7xl`, header, tab rail, and the 3+1 content/sidebar grid — so
 * nothing shifts when the data lands.
 */
export function EditStreamerSkeleton() {
  return (
    <div className="mx-auto max-w-7xl space-y-8 p-4 pb-32 md:p-8 md:pb-32">
      <div className="flex items-center gap-4">
        <Skeleton className="h-10 w-10 rounded-full" />
        <div className="space-y-2">
          <Skeleton className="h-8 w-48" />
          <Skeleton className="h-4 w-64" />
        </div>
      </div>

      <div className="grid grid-cols-1 gap-8 lg:grid-cols-4">
        <div className="space-y-6 lg:col-span-3">
          <Skeleton className="h-11 w-72 rounded-full" />
          <Skeleton className="h-[420px] w-full rounded-xl" />
        </div>
        <div className="space-y-6 lg:col-span-1">
          <Skeleton className="h-40 w-full rounded-xl" />
          <Skeleton className="h-56 w-full rounded-xl" />
        </div>
      </div>
    </div>
  );
}
