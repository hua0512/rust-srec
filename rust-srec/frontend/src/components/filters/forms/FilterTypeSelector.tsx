import { useFormContext } from 'react-hook-form';
import { FormField, FormItem, FormMessage } from '@/components/ui/form';
import { useLingui } from '@lingui/react';
import { Check } from 'lucide-react';
import { cn } from '@/lib/utils';
import { FILTER_TYPES } from '../filter-types';

/**
 * Filter type picker.
 *
 * Built from real `<button role="radio">` elements inside a `radiogroup` rather than clickable
 * `<div>`s, so the options are reachable by keyboard and announced as a single choice.
 */
export function FilterTypeSelector() {
  const { i18n } = useLingui();
  const { control } = useFormContext();

  return (
    <FormField
      control={control}
      name="filter_type"
      render={({ field }) => (
        <FormItem className="space-y-3">
          <div
            role="radiogroup"
            aria-label={i18n._({ id: 'Filter Type', message: 'Filter Type' })}
            className="grid grid-cols-1 gap-2.5 sm:grid-cols-2"
          >
            {FILTER_TYPES.map((type) => {
              const Icon = type.icon;
              const isSelected = field.value === type.value;
              return (
                <button
                  key={type.value}
                  type="button"
                  role="radio"
                  aria-checked={isSelected}
                  onClick={() => field.onChange(type.value)}
                  className={cn(
                    'group relative flex items-start gap-3 rounded-xl border p-3 text-left transition-all',
                    'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2',
                    isSelected
                      ? 'border-primary bg-primary/5 ring-1 ring-primary'
                      : 'border-border/60 hover:border-border hover:bg-muted/40',
                  )}
                >
                  <span
                    className={cn(
                      'flex h-9 w-9 shrink-0 items-center justify-center rounded-lg',
                      type.bg,
                    )}
                  >
                    <Icon className={cn('h-4.5 w-4.5', type.color)} />
                  </span>
                  <span className="min-w-0 flex-1 space-y-0.5">
                    <span className="block text-sm font-semibold leading-tight">
                      {i18n._(type.label)}
                    </span>
                    <span className="block text-xs leading-snug text-muted-foreground">
                      {i18n._(type.description)}
                    </span>
                  </span>
                  {isSelected && (
                    <Check
                      className="h-4 w-4 shrink-0 text-primary"
                      aria-hidden
                    />
                  )}
                </button>
              );
            })}
          </div>
          <FormMessage />
        </FormItem>
      )}
    />
  );
}
