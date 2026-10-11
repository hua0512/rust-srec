import type { ComponentProps, ReactNode } from 'react';
import type { LucideIcon } from 'lucide-react';

import { cn } from '@/lib/utils';

const SIZES = {
  sm: {
    root: 'rounded-md px-2 py-1.5 bg-muted/30 border-transparent group-hover:border-primary/5',
    label: 'gap-1 text-[9px] opacity-50',
    value: 'text-[11px] font-medium text-foreground/80',
  },
  md: {
    root: 'rounded-lg px-3 py-2 bg-muted/40 border-border/50 group-hover:border-primary/10',
    label: 'gap-1.5 text-[10px] font-medium opacity-60',
    value: 'text-xs font-semibold text-foreground/90',
  },
} as const;

function textOf(node: ReactNode): string | undefined {
  return typeof node === 'string' || typeof node === 'number'
    ? String(node)
    : undefined;
}

interface CardFieldProps extends Omit<ComponentProps<'div'>, 'title'> {
  label: ReactNode;
  icon?: LucideIcon;
  value: ReactNode;
  mono?: boolean;
  size?: keyof typeof SIZES;
  /**
   * Tooltip for the whole tile. Without one, the label and the value each show their
   * own text as a tooltip when they are plain strings, since either may be truncated.
   */
  title?: string;
  valueClassName?: string;
}

/** A small "label over value" tile inside a card. Both lines truncate to the tile width. */
export function CardField({
  label,
  icon: Icon,
  value,
  mono = false,
  size = 'sm',
  title,
  className,
  valueClassName,
  ...props
}: CardFieldProps) {
  const styles = SIZES[size];
  const labelText = title ? undefined : textOf(label);
  const valueText = title ? undefined : textOf(value);

  return (
    <div
      title={title}
      className={cn(
        'flex min-w-0 flex-col gap-0.5 border transition-colors',
        styles.root,
        className,
      )}
      {...props}
    >
      <span
        className={cn(
          'flex min-w-0 items-center uppercase tracking-wider',
          styles.label,
        )}
      >
        {Icon && <Icon className="w-3 h-3 shrink-0" />}
        <span className="truncate" title={labelText}>
          {label}
        </span>
      </span>
      <span
        className={cn(
          'truncate',
          styles.value,
          mono && 'font-mono',
          valueClassName,
        )}
        title={valueText}
      >
        {value}
      </span>
    </div>
  );
}
