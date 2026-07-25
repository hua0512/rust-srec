import { ComponentProps, ReactNode } from 'react';
import { CircleHelp } from 'lucide-react';
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from '@/components/ui/tooltip';
import { StatusInfoTooltip } from '@/components/shared/status-info-tooltip';
import { LucideIcon } from 'lucide-react';
import { FormLabel } from '@/components/ui/form';
import { cn } from '@/lib/utils';

/**
 * The field vocabulary used by the platform-specific configuration pages
 * (`config/platforms/tabs/specific-configs/*`): an accented section rule, uppercase tracked
 * labels marked with a small dot, and tall rounded selects.
 *
 * Extracted here so other configuration surfaces can match it without each one carrying its own
 * copy of the class strings.
 */

/**
 * Accent for a settings group.
 *
 * `theme` follows the active theme's primary colour and is the default for new surfaces. The
 * fixed hues exist because the per-platform pages use them to tell their sections apart.
 */
export type ConfigAccent = 'theme' | 'indigo' | 'emerald' | 'sky';

const ACCENT_TEXT: Record<ConfigAccent, string> = {
  theme: 'text-primary',
  indigo: 'text-indigo-500',
  emerald: 'text-emerald-500',
  sky: 'text-sky-500',
};

const ACCENT_DOT: Record<ConfigAccent, string> = {
  theme: 'bg-primary',
  indigo: 'bg-indigo-500',
  emerald: 'bg-emerald-500',
  sky: 'bg-sky-500',
};

/**
 * Class for a `SelectTrigger` so selects line up with the platform pages.
 *
 * `w-full` is load-bearing: the shadcn base is `w-fit`, so a trigger with an empty value would
 * otherwise collapse to just its chevron. `h-11` matches `CONFIG_INPUT`.
 */
export const CONFIG_SELECT_TRIGGER =
  'w-full h-11 bg-background/50 rounded-xl border-border/50 focus:bg-background transition-all shadow-sm';

/** Class for a `SelectContent` dropdown panel. */
export const CONFIG_SELECT_CONTENT = 'rounded-xl border-border/50 shadow-xl';

/** Class for an `Input` so text fields line up with the selects. */
export const CONFIG_INPUT =
  'bg-background/50 h-11 rounded-xl border-border/50 focus:bg-background transition-all shadow-sm';

/** Class for a `FormDescription` under a config field. */
export const CONFIG_DESCRIPTION =
  'text-[11px] font-medium pt-1 px-1 text-muted-foreground/80';

/**
 * The "what does this do?" hint beside a field label.
 *
 * The Tooltip / TooltipContent / StatusInfoTooltip stack underneath is spelled out 27 times
 * across the configuration cards; this is that stack, once.
 */
export function FieldInfo({
  icon,
  title,
  theme = 'blue',
  children,
}: {
  icon: ReactNode;
  title: ReactNode;
  theme?: ComponentProps<typeof StatusInfoTooltip>['theme'];
  children: ReactNode;
}) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <CircleHelp className="h-3.5 w-3.5 cursor-help text-muted-foreground/40 transition-colors hover:text-muted-foreground" />
      </TooltipTrigger>
      {/* `TooltipContent` defaults to `bg-foreground text-background` for short label tips.
          These are full info panels with their own themed header, so they take the normal
          surface colours instead of the inverted ones. */}
      <TooltipContent className="overflow-hidden border border-border/50 bg-popover p-0 text-popover-foreground shadow-xl">
        <StatusInfoTooltip icon={icon} title={title} theme={theme}>
          <p className="text-xs leading-relaxed text-muted-foreground">
            {children}
          </p>
        </StatusInfoTooltip>
      </TooltipContent>
    </Tooltip>
  );
}

/** Accented rule introducing a group of related settings. */
export function ConfigSectionHeading({
  icon: Icon,
  accent = 'theme',
  children,
}: {
  icon: LucideIcon;
  accent?: ConfigAccent;
  children: ReactNode;
}) {
  return (
    <div className="flex items-center gap-3 border-b border-border/40 pb-3">
      <Icon className={cn('h-5 w-5', ACCENT_TEXT[accent])} />
      <h4 className="text-sm font-bold uppercase tracking-[0.2em] text-foreground/80">
        {children}
      </h4>
    </div>
  );
}

const LABEL_CLASS = 'font-bold uppercase tracking-wider text-muted-foreground';

/** `sm` is for labels nested inside an already-labelled group, where the default reads too heavy. */
const LABEL_SIZE = { default: 'text-xs', sm: 'text-[11px]' } as const;

/**
 * Dotted, uppercase field label.
 *
 * Pass `plain` for read-only rows that sit outside a `FormField`, where `FormLabel` has no field
 * context to bind to.
 */
export function ConfigFieldLabel({
  accent = 'theme',
  plain = false,
  size = 'default',
  className,
  children,
}: {
  accent?: ConfigAccent;
  plain?: boolean;
  size?: keyof typeof LABEL_SIZE;
  className?: string;
  children: ReactNode;
}) {
  const labelClass = cn(LABEL_CLASS, LABEL_SIZE[size]);
  const dotSize = size === 'sm' ? 'h-1 w-1' : 'h-1.5 w-1.5';

  return (
    <div className={cn('flex items-center gap-2 px-1', className)}>
      <div className={cn('rounded-full', dotSize, ACCENT_DOT[accent])} />
      {plain ? (
        <span className={labelClass}>{children}</span>
      ) : (
        <FormLabel className={labelClass}>{children}</FormLabel>
      )}
    </div>
  );
}
