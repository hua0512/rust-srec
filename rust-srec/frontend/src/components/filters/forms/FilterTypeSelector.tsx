import { useFormContext } from 'react-hook-form';
import { FormField, FormItem, FormLabel, FormMessage } from '../../ui/form';
import { Trans } from '@lingui/macro';
import { Clock, Tag, Folder, Calendar, Regex } from 'lucide-react';
import { cn } from '@/lib/utils';
import { t } from '@lingui/macro';

const FILTER_TYPES = [
  {
    value: 'KEYWORD',
    label: t`Keyword`,
    icon: Tag,
    description: t`Filter by title keywords`,
    color: 'text-emerald-500',
    bg: 'bg-emerald-500/10',
    border: 'peer-data-[state=checked]:border-emerald-500',
  },
  {
    value: 'TIME_BASED',
    label: t`Time Based`,
    icon: Clock,
    description: t`Schedule recording times`,
    color: 'text-blue-500',
    bg: 'bg-blue-500/10',
    border: 'peer-data-[state=checked]:border-blue-500',
  },
  {
    value: 'CATEGORY',
    label: t`Category`,
    icon: Folder,
    description: t`Filter by game/category`,
    color: 'text-violet-500',
    bg: 'bg-violet-500/10',
    border: 'peer-data-[state=checked]:border-violet-500',
  },
  {
    value: 'CRON',
    label: t`Cron`,
    icon: Calendar,
    description: t`Advanced scheduling`,
    color: 'text-orange-500',
    bg: 'bg-orange-500/10',
    border: 'peer-data-[state=checked]:border-orange-500',
  },
  {
    value: 'REGEX',
    label: t`Regex`,
    icon: Regex,
    description: t`Complex patterns`,
    color: 'text-pink-500',
    bg: 'bg-pink-500/10',
    border: 'peer-data-[state=checked]:border-pink-500',
  },
];

export function FilterTypeSelector() {
  const { control } = useFormContext();

  return (
    <FormField
      control={control}
      name="filter_type"
      render={({ field }) => (
        <FormItem className="space-y-3">
          <FormLabel className="text-base font-semibold">
            <Trans>Filter Type</Trans>
          </FormLabel>
          <div className="grid grid-cols-2 md:grid-cols-3 gap-3">
            {FILTER_TYPES.map((type) => {
              const Icon = type.icon;
              const isSelected = field.value === type.value;
              return (
                <div
                  key={type.value}
                  onClick={() => field.onChange(type.value)}
                  className={cn(
                    'cursor-pointer rounded-xl border-2 p-4 transition-all hover:bg-muted/50 relative',
                    isSelected
                      ? `border-primary bg-primary/5 shadow-md ${type.border.replace('peer-data-[state=checked]:', '')}`
                      : 'border-muted bg-card',
                  )}
                >
                  <div
                    className={cn(
                      'mb-2 w-8 h-8 rounded-lg flex items-center justify-center',
                      type.bg,
                    )}
                  >
                    <Icon className={cn('w-5 h-5', type.color)} />
                  </div>
                  <div className="space-y-1">
                    <div className="font-semibold text-sm leading-none">
                      {type.label}
                    </div>
                    <div className="text-xs text-muted-foreground line-clamp-1">
                      {type.description}
                    </div>
                  </div>
                  {isSelected && (
                    <div className="absolute top-2 right-2 w-2 h-2 rounded-full bg-primary" />
                  )}
                </div>
              );
            })}
          </div>
          <FormMessage />
        </FormItem>
      )}
    />
  );
}
