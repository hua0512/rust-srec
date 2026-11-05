import type { ComponentProps, ReactNode } from 'react';
import type { LucideIcon } from 'lucide-react';
import { cn } from '@/lib/utils';

/** The rounded track a row of `FilterPill`s sits in. */
export const FILTER_PILL_GROUP =
  'flex items-center rounded-full border border-border/50 bg-muted/30 p-1 shrink-0';

/**
 * One option of a segmented filter inside a `FILTER_PILL_GROUP` track. Extra
 * button props pass through, so it can also be a popover trigger.
 */
export function FilterPill({
  icon: Icon,
  activeIconClassName = 'text-primary',
  isActive,
  className,
  children,
  ...props
}: ComponentProps<'button'> & {
  icon?: LucideIcon;
  /** The icon's colour while the pill is selected. */
  activeIconClassName?: string;
  isActive: boolean;
}) {
  return (
    <button
      type="button"
      aria-pressed={isActive}
      className={cn(
        'flex items-center gap-1.5 whitespace-nowrap rounded-full px-3 py-1.5 text-xs font-medium transition-all duration-200',
        isActive
          ? 'bg-background text-foreground shadow-sm ring-1 ring-border/50'
          : 'text-muted-foreground hover:bg-muted/50 hover:text-foreground',
        className,
      )}
      {...props}
    >
      {Icon && (
        <Icon
          className={cn(
            'h-3 w-3',
            isActive ? activeIconClassName : 'text-muted-foreground',
          )}
        />
      )}
      <span>{children}</span>
    </button>
  );
}

/**
 * One standalone chip in a list page's filter row. `count` arrives
 * pre-formatted and is omitted while it is unknown, so the chip never briefly
 * claims zero.
 */
export function FilterChip({
  icon: Icon,
  label,
  count,
  isActive,
  onClick,
}: {
  icon: LucideIcon;
  label: ReactNode;
  count?: string;
  isActive: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-pressed={isActive}
      className={cn(
        'relative flex shrink-0 items-center gap-1.5 rounded-full px-3.5 py-1.5 text-xs font-medium shadow-sm ring-1 transition-all duration-200 sm:text-sm',
        isActive
          ? 'bg-primary text-primary-foreground ring-primary'
          : 'bg-background text-muted-foreground ring-border/50 hover:bg-muted hover:text-foreground',
      )}
    >
      <Icon className="h-4 w-4" />
      <span className="whitespace-nowrap">{label}</span>
      {count !== undefined && (
        <span className="tabular-nums opacity-70">{count}</span>
      )}
    </button>
  );
}
