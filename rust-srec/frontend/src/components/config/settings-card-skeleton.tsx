import { ReactNode } from 'react';
import { Skeleton } from '@/components/ui/skeleton';
import { cn } from '@/lib/utils';

interface SettingsCardSkeletonProps {
  /**
   * Field rows per section, in order. A single entry describes a card that lists its fields
   * flat; two or more draw a `ConfigSectionHeading`-sized bar per entry and a `Separator`
   * between them, matching how `ConcurrencyCard` and `NetworkSystemCard` group their fields.
   *
   * Count rows, not fields: fields sitting in a `@md:grid-cols-2` grid share a row.
   */
  sections: number[];
  /** Replaces the generated body, for cards whose content is not a list of fields. */
  children?: ReactNode;
  className?: string;
}

/**
 * Loading placeholder shaped like `SettingsCard`.
 *
 * Mirrors that component's spacing so the page keeps its height when the lazy card chunks
 * resolve. Heights here track the real controls: the 40px header chip is `SettingsCard`'s
 * `p-2.5` around a 20px icon, and each field row is a `text-sm` label over an `h-9` input.
 */
export function SettingsCardSkeleton({
  sections,
  children,
  className,
}: SettingsCardSkeletonProps) {
  const sectioned = sections.length > 1;

  return (
    <div
      aria-hidden
      className={cn(
        'h-full rounded-xl border border-white/10 bg-background/30 py-6 shadow-xl',
        className,
      )}
    >
      <div className="flex items-center gap-3 px-6">
        <Skeleton className="h-10 w-10 rounded-xl" />
        <div className="space-y-2">
          <Skeleton className="h-4 w-44" />
          <Skeleton className="h-3 w-60" />
        </div>
      </div>

      <div className="space-y-8 px-6 pt-8">
        {children ??
          sections.map((rows, section) => (
            <div key={section} className="space-y-4">
              {sectioned && section > 0 && (
                <Skeleton className="h-px w-full rounded-none" />
              )}
              {sectioned && <Skeleton className="h-4 w-36" />}
              <div className="space-y-6">
                {Array.from({ length: rows }, (_, row) => (
                  <SettingsFieldSkeleton key={row} />
                ))}
              </div>
            </div>
          ))}
      </div>
    </div>
  );
}

/** One label-over-control pair, at the heights `FormItem` gives them. */
export function SettingsFieldSkeleton() {
  return (
    <div className="space-y-2">
      <Skeleton className="h-3.5 w-28" />
      <Skeleton className="h-9 w-full rounded-md" />
    </div>
  );
}
