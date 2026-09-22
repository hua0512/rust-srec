import { useRef, type KeyboardEvent, type ReactNode } from 'react';
import type { LucideIcon } from 'lucide-react';
import { cn } from '@/lib/utils';

export interface OptionGroupItem<T extends string> {
  value: T;
  label: string;
  hint?: string;
  icon?: LucideIcon;
  disabled?: boolean;
}

const gridColumns = { 2: 'grid-cols-2', 3: 'grid-cols-3' };

/**
 * Single-choice tile grid rendered as a `radiogroup`, so screen readers
 * announce it as one choice and Arrow keys move between enabled options.
 */
export function OptionGroup<T extends string>({
  value,
  options,
  onValueChange,
  columns = 3,
  disabled = false,
  className,
  ...aria
}: {
  value: T;
  options: OptionGroupItem<T>[];
  onValueChange: (value: T) => void;
  columns?: keyof typeof gridColumns;
  disabled?: boolean;
  className?: string;
  'aria-labelledby'?: string;
  'aria-describedby'?: string;
}) {
  const refs = useRef(new Map<T, HTMLButtonElement | null>());
  const enabled = options.filter((option) => !option.disabled);
  // Keep one option tabbable even when the current value is not in the list.
  const focusable = options.some((option) => option.value === value)
    ? value
    : enabled[0]?.value;

  const handleKeyDown = (event: KeyboardEvent<HTMLButtonElement>) => {
    const forward = event.key === 'ArrowRight' || event.key === 'ArrowDown';
    const backward = event.key === 'ArrowLeft' || event.key === 'ArrowUp';
    if ((!forward && !backward) || enabled.length === 0) return;
    event.preventDefault();
    const index = enabled.findIndex((option) => option.value === value);
    const offset = forward ? 1 : enabled.length - 1;
    const next = enabled[(index + offset) % enabled.length].value;
    onValueChange(next);
    refs.current.get(next)?.focus();
  };

  return (
    <div
      role="radiogroup"
      {...aria}
      aria-disabled={disabled || undefined}
      className={cn('grid gap-1.5', gridColumns[columns], className)}
    >
      {options.map((option) => {
        const checked = option.value === value;
        const Icon = option.icon;
        return (
          <button
            key={option.value}
            ref={(node) => {
              refs.current.set(option.value, node);
            }}
            type="button"
            role="radio"
            aria-checked={checked}
            tabIndex={option.value === focusable ? 0 : -1}
            disabled={disabled || option.disabled}
            onClick={() => onValueChange(option.value)}
            onKeyDown={handleKeyDown}
            title={option.label}
            className={cn(
              'flex min-w-0 flex-col items-center justify-center gap-1 rounded-lg border px-2 py-1.5 text-center text-xs transition-all',
              Icon || option.hint ? 'min-h-12' : 'min-h-8',
              'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring',
              'disabled:cursor-not-allowed disabled:opacity-40',
              checked
                ? 'border-primary/60 bg-primary/10 text-foreground shadow-sm ring-1 ring-primary/30'
                : 'border-border/60 bg-background/40 text-muted-foreground hover:border-border hover:bg-accent/50 hover:text-foreground',
            )}
          >
            {Icon && (
              <Icon
                className={cn(
                  'h-4 w-4 shrink-0 transition-colors',
                  checked ? 'text-primary' : 'text-muted-foreground/70',
                )}
              />
            )}
            {/* Long CDN hosts and localized quality names wrap instead of hiding. */}
            <span className="max-w-full font-medium leading-tight [overflow-wrap:anywhere]">
              {option.label}
            </span>
            {option.hint && (
              <span className="max-w-full truncate text-[10px] leading-tight text-muted-foreground tabular-nums">
                {option.hint}
              </span>
            )}
          </button>
        );
      })}
    </div>
  );
}

export function SettingsSection({
  id,
  icon: Icon,
  title,
  aside,
  children,
  footer,
}: {
  id: string;
  icon: LucideIcon;
  title: ReactNode;
  aside?: ReactNode;
  children: ReactNode;
  footer?: ReactNode;
}) {
  return (
    <section className="space-y-2">
      <div className="flex items-center justify-between gap-2">
        <h4
          id={id}
          className="flex items-center gap-1.5 text-xs font-semibold text-foreground/80"
        >
          <Icon className="h-3.5 w-3.5 text-muted-foreground" />
          {title}
        </h4>
        {aside != null && (
          <span
            aria-hidden
            className="text-[10px] tabular-nums text-muted-foreground"
          >
            {aside}
          </span>
        )}
      </div>
      {children}
      {footer && (
        <p
          id={`${id}-help`}
          className="rounded-md bg-muted/50 px-2.5 py-2 text-[11px] leading-relaxed text-muted-foreground"
        >
          {footer}
        </p>
      )}
    </section>
  );
}
