import { Fragment, type ElementType, type ReactNode } from 'react';
import { plural, t } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import type { UseQueryResult } from '@tanstack/react-query';
import { AlertCircle, ChevronDown } from 'lucide-react';
import { Callout } from '@/components/shared/callout';
import { Skeleton } from '@/components/ui/skeleton';
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from '@/components/ui/tooltip';
import { cn } from '@/lib/utils';

/** The divider between the bands of a `ListPanel`. */
export const PANEL_BAND = 'border-t border-border/50';

/** A compact button beside a row's menu. */
export const ROW_BUTTON = 'h-7 gap-1 rounded-lg px-2 text-xs has-[>svg]:px-2';

/**
 * A titled, bordered list of things such as accounts or proxies: a header with
 * the title, a short description and an optional action, then its bands, each
 * starting with `PANEL_BAND`.
 */
export function ListPanel({
  title,
  description,
  action,
  children,
}: {
  title: ReactNode;
  description: ReactNode;
  action?: ReactNode;
  children: ReactNode;
}) {
  return (
    <section className="@container overflow-hidden rounded-xl border border-border/50 bg-card">
      {/* A narrow panel wraps the action under the title instead of squeezing
          the description into a sliver beside it. */}
      <header className="flex flex-wrap items-center gap-x-3 gap-y-2 px-3 py-3 sm:px-4">
        <div className="min-w-0 flex-1 basis-56 space-y-0.5">
          <h4 className="text-sm font-semibold">{title}</h4>
          <div className="text-xs text-muted-foreground">{description}</div>
        </div>
        {action && <div className="shrink-0">{action}</div>}
      </header>
      {children}
    </section>
  );
}

/** Placeholder rows shaped like a list row while the list loads. */
export function ListPanelSkeleton() {
  return (
    <ul aria-hidden="true" className="divide-y divide-border/50">
      {[0, 1].map((index) => (
        <li key={index} className="flex items-center gap-3 px-3 py-2.5 sm:px-4">
          <Skeleton className="size-9 shrink-0 rounded-full" />
          <div className="min-w-0 flex-1 space-y-1.5">
            <Skeleton className="h-3.5 w-32 max-w-full" />
            <Skeleton className="h-3 w-56 max-w-full" />
          </div>
          <Skeleton className="h-3 w-10 shrink-0" />
        </li>
      ))}
    </ul>
  );
}

/** What an empty list shows in place of its rows. */
export function ListPanelEmpty({
  icon: Icon,
  title,
  help,
  action,
}: {
  icon: ElementType;
  title: ReactNode;
  help: ReactNode;
  action?: ReactNode;
}) {
  return (
    <div className="flex flex-col items-center gap-1 px-4 py-8 text-center">
      <div className="mb-2 rounded-full bg-muted/60 p-3">
        <Icon className="size-5 text-muted-foreground" />
      </div>
      <p className="text-sm font-medium">{title}</p>
      <p className="max-w-sm text-xs text-muted-foreground">{help}</p>
      {action && <div className="mt-3">{action}</div>}
    </div>
  );
}

/**
 * A list's rows as the last band of a `ListPanel`, with its loading, error and
 * empty states.
 */
export function ListPanelRows<T>({
  query,
  renderRow,
  empty,
}: {
  query: Pick<UseQueryResult<T[]>, 'data' | 'error' | 'isPending'>;
  renderRow: (item: T) => ReactNode;
  /** Shown when the list has no rows. */
  empty: ReactNode;
}) {
  return (
    <div className={PANEL_BAND}>
      {query.error && (
        <div className="px-3 py-3 sm:px-4">
          <Callout tone="error" icon={AlertCircle}>
            {query.error.message}
          </Callout>
        </div>
      )}
      {query.isPending && <ListPanelSkeleton />}
      {query.data?.length === 0 && empty}
      {query.data && query.data.length > 0 && (
        <ul className="divide-y divide-border/50">
          {query.data.map(renderRow)}
        </ul>
      )}
    </div>
  );
}

/**
 * The chevron that expands a row. Its hit area stretches over the whole row,
 * so a click anywhere but on the row's actions, which sit above it with
 * `relative z-10`, expands it. The row needs `relative`.
 */
export function RowExpandButton({
  open,
  onToggle,
  controls,
  label,
}: {
  open: boolean;
  onToggle: () => void;
  controls: string;
  label: string;
}) {
  return (
    <button
      type="button"
      aria-expanded={open}
      aria-controls={controls}
      aria-label={label}
      data-state={open ? 'open' : 'closed'}
      onClick={onToggle}
      className="rs-disclosure-trigger flex size-7 shrink-0 items-center justify-center rounded-lg text-muted-foreground outline-none after:absolute after:inset-0 after:content-[''] hover:text-foreground focus-visible:ring-[3px] focus-visible:ring-ring/50 @md:size-8"
    >
      <span className="rs-disclosure-chevron">
        <ChevronDown className="size-4" />
      </span>
    </button>
  );
}

/**
 * Short phrases joined by " · ", each kept whole on its line so a wrapping
 * line breaks only after a separator. A phrase wider than the line truncates,
 * leaving room for the separator after it.
 */
export function DotSeparated({
  parts,
}: {
  parts: { content: ReactNode; className?: string }[];
}) {
  return parts.map((part, index) => (
    <Fragment key={index}>
      {/* The no-break space keeps each "·" at the end of its line. */}
      {index > 0 && '\u00a0· '}
      <span
        className={cn(
          'inline-block truncate align-bottom',
          index < parts.length - 1 ? 'max-w-[calc(100%-1em)]' : 'max-w-full',
          part.className,
        )}
      >
        {part.content}
      </span>
    </Fragment>
  ));
}

/** Names a dialog lists per group before the rest collapse into "+N more". */
export const DIALOG_NAME_LIMIT = 8;

/** Up to `limit` names, then a "+N more" whose tooltip lists the rest. */
export function NameList<T>({
  items,
  limit,
  render,
  label,
}: {
  items: T[];
  limit: number;
  render: (item: T, index: number) => ReactNode;
  label: (item: T) => string;
}) {
  const { i18n } = useLingui();
  const shown = items.slice(0, limit);
  const rest = items.slice(limit);
  return (
    <>
      {/* Each name keeps its icon and its trailing comma on its line; a name
          too long for the row truncates instead of breaking. */}
      {shown.map((item, index) => (
        <span key={index}>
          {index > 0 && ' '}
          <span className="inline-flex max-w-full items-baseline align-bottom">
            {render(item, index)}
            {index < shown.length - 1 && ','}
          </span>
        </span>
      ))}
      {rest.length > 0 && (
        <>
          {' '}
          <Tooltip>
            <TooltipTrigger asChild>
              <span className="cursor-help whitespace-nowrap underline decoration-dotted underline-offset-2">
                {t(i18n)`${plural(rest.length, { other: '+# more' })}`}
              </span>
            </TooltipTrigger>
            <TooltipContent>{rest.map(label).join(', ')}</TooltipContent>
          </Tooltip>
        </>
      )}
    </>
  );
}

/** The style of a name that links to where it is configured. */
export const NAME_LINK =
  'min-w-0 truncate font-medium text-foreground/80 hover:underline';
