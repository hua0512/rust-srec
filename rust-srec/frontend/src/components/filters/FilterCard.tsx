import {
  FilterSchema,
  TimeBasedFilterConfigSchema,
  KeywordFilterConfigSchema,
  CronFilterConfigSchema,
  RegexFilterConfigSchema,
  type FilterType,
  normalizeFilterConfigForType,
} from '../../api/schemas';
import { z } from 'zod';
import { Card, CardContent, CardHeader } from '../ui/card';
import { Badge } from '../ui/badge';
import { Button } from '../ui/button';
import { AlertTriangle, Pencil, Trash2 } from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { cn } from '@/lib/utils';
import { filterTypeMeta } from './filter-types';

type Filter = z.infer<typeof FilterSchema>;

interface FilterCardProps {
  filter: Filter;
  onEdit: (filter: Filter) => void;
  onDelete: (filterId: string) => void;
}

/**
 * Day initials for the time-window summary.
 *
 * Two pairs collide in English (Tuesday/Thursday, Saturday/Sunday), so each carries a `context`
 * to keep them separate message ids and independently translatable.
 */
const DAY_INITIALS = [
  { id: 'Monday', label: msg({ message: 'M', context: 'Monday initial' }) },
  { id: 'Tuesday', label: msg({ message: 'T', context: 'Tuesday initial' }) },
  {
    id: 'Wednesday',
    label: msg({ message: 'W', context: 'Wednesday initial' }),
  },
  { id: 'Thursday', label: msg({ message: 'T', context: 'Thursday initial' }) },
  { id: 'Friday', label: msg({ message: 'F', context: 'Friday initial' }) },
  { id: 'Saturday', label: msg({ message: 'S', context: 'Saturday initial' }) },
  { id: 'Sunday', label: msg({ message: 'S', context: 'Sunday initial' }) },
];

export function FilterCard({ filter, onEdit, onDelete }: FilterCardProps) {
  const { i18n } = useLingui();
  const meta = filterTypeMeta(filter.filter_type);
  const Icon = meta?.icon;

  const renderConfig = () => {
    switch (filter.filter_type) {
      case 'TIME_BASED': {
        const config = TimeBasedFilterConfigSchema.safeParse(
          normalizeFilterConfigForType(
            filter.filter_type as FilterType,
            filter.config,
          ),
        );
        if (!config.success) return <InvalidConfig />;
        const { days_of_week, start_time, end_time } = config.data;

        return (
          <div className="space-y-3">
            <div className="flex gap-1">
              {DAY_INITIALS.map((day) => {
                const isActive = days_of_week.includes(day.id as any);
                return (
                  <span
                    key={day.id}
                    title={day.id}
                    className={cn(
                      'flex h-5 w-5 items-center justify-center rounded-full text-[10px] font-bold',
                      isActive
                        ? 'bg-primary text-primary-foreground'
                        : 'bg-muted text-muted-foreground/60',
                    )}
                  >
                    {i18n._(day.label)}
                  </span>
                );
              })}
            </div>
            <p className="font-mono text-sm tabular-nums">
              {start_time.slice(0, 5)}
              <span className="px-1.5 text-muted-foreground">–</span>
              {end_time.slice(0, 5)}
            </p>
          </div>
        );
      }

      case 'KEYWORD': {
        const config = KeywordFilterConfigSchema.safeParse(
          normalizeFilterConfigForType(
            filter.filter_type as FilterType,
            filter.config,
          ),
        );
        if (!config.success) return <InvalidConfig />;
        const { include, exclude } = config.data;

        if (include.length === 0 && exclude.length === 0) {
          return (
            <p className="text-sm text-muted-foreground">
              <Trans>No keywords set</Trans>
            </p>
          );
        }

        return (
          <div className="space-y-3">
            {include.length > 0 && (
              <KeywordGroup
                title={<Trans>Include</Trans>}
                words={include}
                tone="include"
              />
            )}
            {exclude.length > 0 && (
              <KeywordGroup
                title={<Trans>Exclude</Trans>}
                words={exclude}
                tone="exclude"
              />
            )}
          </div>
        );
      }

      case 'CRON': {
        const config = CronFilterConfigSchema.safeParse(filter.config);
        if (!config.success) return <InvalidConfig />;
        return (
          <div className="space-y-2">
            <code className="block rounded-lg border bg-muted/50 px-3 py-2 font-mono text-sm">
              {config.data.expression}
            </code>
            {config.data.timezone && (
              <p className="text-xs text-muted-foreground">
                {config.data.timezone}
              </p>
            )}
          </div>
        );
      }

      case 'REGEX': {
        const config = RegexFilterConfigSchema.safeParse(filter.config);
        if (!config.success) return <InvalidConfig />;
        return (
          <div className="space-y-3">
            <div className="flex flex-wrap items-center gap-2">
              <Badge
                variant={config.data.exclude ? 'destructive' : 'secondary'}
                className="h-5 rounded-md px-2 text-[10px]"
              >
                {config.data.exclude ? (
                  <Trans>Exclude</Trans>
                ) : (
                  <Trans>Include</Trans>
                )}
              </Badge>
              {config.data.case_insensitive && (
                <Badge
                  variant="outline"
                  className="h-5 rounded-md px-2 text-[10px] font-normal text-muted-foreground"
                >
                  <Trans>Ignore case</Trans>
                </Badge>
              )}
            </div>
            <code className="block rounded-lg border bg-muted/50 px-3 py-2 font-mono text-xs leading-relaxed break-all">
              {config.data.pattern || (
                <span className="text-muted-foreground italic">
                  <Trans>Empty pattern</Trans>
                </span>
              )}
            </code>
          </div>
        );
      }

      default:
        return (
          <p className="text-sm text-muted-foreground">
            <Trans>Unknown filter type</Trans>
          </p>
        );
    }
  };

  return (
    <Card className="group flex h-full flex-col border-border/50 shadow-sm transition-all hover:shadow-md">
      <CardHeader className="pb-3">
        <div className="flex items-start justify-between gap-2">
          <div className="flex items-center gap-2.5">
            {Icon && (
              <span
                className={cn('rounded-lg p-1.5', meta.bg, meta.color)}
                aria-hidden
              >
                <Icon className="h-4 w-4" />
              </span>
            )}
            <span className="text-sm font-semibold">
              {meta ? i18n._(meta.label) : filter.filter_type}
            </span>
          </div>

          {/* Kept at reduced opacity rather than hidden: hover-only actions are unreachable on
              touch and undiscoverable with a keyboard. */}
          <div className="flex gap-1 opacity-60 transition-opacity group-hover:opacity-100 focus-within:opacity-100">
            <Button
              type="button"
              variant="ghost"
              size="icon"
              className="h-7 w-7"
              onClick={() => onEdit(filter)}
            >
              <Pencil className="h-3.5 w-3.5" />
              <span className="sr-only">
                <Trans>Edit filter</Trans>
              </span>
            </Button>
            <Button
              type="button"
              variant="ghost"
              size="icon"
              className="h-7 w-7 text-muted-foreground hover:bg-destructive/10 hover:text-destructive"
              onClick={() => onDelete(filter.id)}
            >
              <Trash2 className="h-3.5 w-3.5" />
              <span className="sr-only">
                <Trans>Delete filter</Trans>
              </span>
            </Button>
          </div>
        </div>
      </CardHeader>
      <CardContent className="flex-1">{renderConfig()}</CardContent>
    </Card>
  );
}

/** Shown when a stored config no longer matches its type's schema. */
function InvalidConfig() {
  return (
    <p className="flex items-center gap-1.5 text-sm text-destructive">
      <AlertTriangle className="h-3.5 w-3.5 shrink-0" />
      <Trans>This filter's settings could not be read.</Trans>
    </p>
  );
}

function KeywordGroup({
  title,
  words,
  tone,
}: {
  title: React.ReactNode;
  words: string[];
  tone: 'include' | 'exclude';
}) {
  return (
    <div className="space-y-1.5">
      <p className="text-[11px] font-bold uppercase tracking-wider text-muted-foreground">
        {title}
      </p>
      <div className="flex flex-wrap gap-1.5">
        {words.map((word) => (
          <Badge
            key={word}
            variant={tone === 'exclude' ? 'destructive' : 'secondary'}
            className="h-5 px-1.5 text-[11px] font-normal"
          >
            {word}
          </Badge>
        ))}
      </div>
    </div>
  );
}
