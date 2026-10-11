import type { ComponentProps, ElementType, ReactNode } from 'react';
import { MoreHorizontal } from 'lucide-react';
import { Trans } from '@lingui/react/macro';

import { Button } from '@/components/ui/button';
import { Card, CardFooter, CardHeader, CardTitle } from '@/components/ui/card';
import { cn } from '@/lib/utils';

const ACCENTS = {
  primary: {
    line: 'via-primary/40',
    glow: 'from-primary/5',
  },
  purple: {
    line: 'via-purple-500/40',
    glow: 'from-purple-500/5',
  },
} as const;

export type CardAccent = keyof typeof ACCENTS;

interface CardDecorationProps {
  accent?: CardAccent;
  /** The gradient line along the top edge that appears on hover. */
  line?: boolean;
}

/**
 * The hover line and glow of a decorated card. The parent must be `relative`,
 * `overflow-hidden` and a `group`; content drawn above it needs `relative z-10`.
 */
export function CardDecoration({
  accent = 'primary',
  line = true,
}: CardDecorationProps) {
  const colors = ACCENTS[accent];
  return (
    <>
      {line && (
        <div
          className={cn(
            'absolute inset-x-0 top-0 h-0.5 bg-gradient-to-r from-transparent to-transparent opacity-0 group-hover:opacity-100 transition-opacity duration-700',
            colors.line,
          )}
        />
      )}
      <div
        className={cn(
          'absolute -inset-0.5 bg-gradient-to-br to-transparent opacity-0 group-hover:opacity-100 blur-2xl transition-opacity duration-500 pointer-events-none',
          colors.glow,
        )}
      />
    </>
  );
}

interface DecoratedCardProps extends ComponentProps<typeof Card> {
  accent?: CardAccent;
}

/** A full-height grid card that lifts and glows on hover. */
export function DecoratedCard({
  accent,
  className,
  children,
  ...props
}: DecoratedCardProps) {
  return (
    <Card
      className={cn(
        'relative h-full flex flex-col transition-all duration-500 hover:-translate-y-1 hover:shadow-2xl hover:shadow-primary/10 group overflow-hidden bg-gradient-to-br from-background/80 to-background/40 border-border/40 hover:border-primary/20',
        className,
      )}
      {...props}
    >
      <CardDecoration accent={accent} />
      {children}
    </Card>
  );
}

interface CardIconTileProps {
  icon: ElementType;
  /** Tile colours, usually `bg-*` and `text-*`; the icon inherits the text colour. */
  className?: string;
  iconClassName?: string;
}

export function CardIconTile({
  icon: Icon,
  className,
  iconClassName,
}: CardIconTileProps) {
  return (
    <div
      className={cn(
        'p-3 rounded-2xl shrink-0 ring-1 ring-inset ring-black/5 dark:ring-white/5 transition-transform duration-500 group-hover:scale-110 group-hover:rotate-3',
        className,
      )}
    >
      <Icon className={cn('h-5 w-5', iconClassName)} />
    </div>
  );
}

interface CardHeaderRowProps extends Omit<
  ComponentProps<typeof CardHeader>,
  'title'
> {
  /** Leading element, usually a `CardIconTile`. */
  icon: ReactNode;
  /** Truncates to one line; a string title is also its own tooltip. */
  title: ReactNode;
  titleClassName?: string;
  /** Small uppercase line under the title; a string truncates. */
  subtitle?: ReactNode;
  /** Trailing elements such as a badge or the card menu. */
  actions?: ReactNode;
}

/** Icon, title over subtitle, and actions in one row; only the title column shrinks. */
export function CardHeaderRow({
  icon,
  title,
  titleClassName,
  subtitle,
  actions,
  className,
  ...props
}: CardHeaderRowProps) {
  return (
    <CardHeader
      className={cn(
        'relative flex flex-row items-center gap-4 pb-2 space-y-0 z-10',
        className,
      )}
      {...props}
    >
      {icon}
      <div className="flex-1 min-w-0 space-y-1">
        <CardTitle
          className={cn(
            'text-base font-medium truncate tracking-tight text-foreground/90 group-hover:text-primary transition-colors duration-300',
            titleClassName,
          )}
          title={typeof title === 'string' ? title : undefined}
        >
          {title}
        </CardTitle>
        {subtitle != null && subtitle !== false && (
          <div className="flex min-w-0 items-center gap-2 text-[10px] uppercase tracking-wider font-semibold text-muted-foreground/60">
            {typeof subtitle === 'string' ? (
              <span className="truncate" title={subtitle}>
                {subtitle}
              </span>
            ) : (
              subtitle
            )}
          </div>
        )}
      </div>
      {actions}
    </CardHeader>
  );
}

/** The `...` trigger of a card's dropdown menu; use inside `DropdownMenuTrigger asChild`. */
export function CardMenuButton({
  className,
  ...props
}: ComponentProps<typeof Button>) {
  return (
    <Button
      variant="ghost"
      size="icon"
      className={cn(
        'h-8 w-8 -mr-2 text-muted-foreground/40 hover:text-foreground transition-colors',
        className,
      )}
      {...props}
    >
      <MoreHorizontal className="h-4 w-4" />
      <span className="sr-only">
        <Trans>Open menu</Trans>
      </span>
    </Button>
  );
}

/** The muted strip of small metadata at the bottom of a card. */
export function CardMetaFooter({
  className,
  ...props
}: ComponentProps<typeof CardFooter>) {
  return (
    <CardFooter
      className={cn(
        'relative pt-0 text-[10px] text-muted-foreground flex justify-between items-center z-10 border-t border-border/20 mt-auto px-6 py-3 bg-muted/5',
        className,
      )}
      {...props}
    />
  );
}
