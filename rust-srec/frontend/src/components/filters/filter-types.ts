import { Calendar, Clock, Regex, Tag, type LucideIcon } from 'lucide-react';
import { msg } from '@lingui/core/macro';
import type { MessageDescriptor } from '@lingui/core';
import type { z } from 'zod';
import type { DaysOfWeekSchema } from '@/api/schemas';

export interface FilterTypeMeta {
  value: string;
  /** Resolved with `i18n._()` at render; these live at module scope. */
  label: MessageDescriptor;
  description: MessageDescriptor;
  icon: LucideIcon;
  /** Accent for the icon glyph. */
  color: string;
  /** Accent for the icon chip behind the glyph. */
  bg: string;
  /** Selected-state border, used by the type picker's radio cards. */
  border: string;
}

/**
 * One description of each filter type, shared by the type picker in the create dialog and the
 * cards that list existing filters, so a type's name, icon and accent cannot disagree between
 * the two places a user sees it.
 */
export const FILTER_TYPES: FilterTypeMeta[] = [
  {
    value: 'KEYWORD',
    label: msg`Keyword`,
    description: msg`Filter by title keywords`,
    icon: Tag,
    color: 'text-emerald-500',
    bg: 'bg-emerald-500/10',
    border: 'peer-data-[state=checked]:border-emerald-500',
  },
  {
    value: 'TIME_BASED',
    label: msg`Time Based`,
    description: msg`Schedule recording times`,
    icon: Clock,
    color: 'text-blue-500',
    bg: 'bg-blue-500/10',
    border: 'peer-data-[state=checked]:border-blue-500',
  },
  {
    value: 'CRON',
    label: msg`Cron`,
    description: msg`Advanced scheduling`,
    icon: Calendar,
    color: 'text-orange-500',
    bg: 'bg-orange-500/10',
    border: 'peer-data-[state=checked]:border-orange-500',
  },
  {
    value: 'REGEX',
    label: msg`Regex`,
    description: msg`Complex patterns`,
    icon: Regex,
    color: 'text-pink-500',
    bg: 'bg-pink-500/10',
    border: 'peer-data-[state=checked]:border-pink-500',
  },
];

export function filterTypeMeta(value: string): FilterTypeMeta | undefined {
  return FILTER_TYPES.find((t) => t.value === value);
}

/**
 * `id` is the wire value stored in `config.days_of_week` and must stay English; `label` and
 * `full` are display only.
 *
 * The initials collide in pairs (Tuesday/Thursday, Saturday/Sunday), so each carries a `context`
 * to keep them separate message ids.
 */
export const DAYS: {
  id: z.infer<typeof DaysOfWeekSchema>;
  label: MessageDescriptor;
  full: MessageDescriptor;
}[] = [
  {
    id: 'Monday',
    label: msg({ message: 'M', context: 'Monday initial' }),
    full: msg`Monday`,
  },
  {
    id: 'Tuesday',
    label: msg({ message: 'T', context: 'Tuesday initial' }),
    full: msg`Tuesday`,
  },
  {
    id: 'Wednesday',
    label: msg({ message: 'W', context: 'Wednesday initial' }),
    full: msg`Wednesday`,
  },
  {
    id: 'Thursday',
    label: msg({ message: 'T', context: 'Thursday initial' }),
    full: msg`Thursday`,
  },
  {
    id: 'Friday',
    label: msg({ message: 'F', context: 'Friday initial' }),
    full: msg`Friday`,
  },
  {
    id: 'Saturday',
    label: msg({ message: 'S', context: 'Saturday initial' }),
    full: msg`Saturday`,
  },
  {
    id: 'Sunday',
    label: msg({ message: 'S', context: 'Sunday initial' }),
    full: msg`Sunday`,
  },
];
