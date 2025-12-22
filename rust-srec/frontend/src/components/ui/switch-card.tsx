import * as React from 'react';
import { FormControl, FormItem, FormLabel } from './form';
import { Switch } from './switch';
import { cn } from '@/lib/utils';

export interface SwitchCardProps {
  label: React.ReactNode;
  description?: React.ReactNode;
  checked: boolean;
  onCheckedChange: (checked: boolean) => void;
  className?: string;
  disabled?: boolean;
}

/**
 * A card-style switch toggle with label and optional description.
 * Reduces duplication of the "flex row items-center justify-between border rounded-lg p-3" pattern.
 */
const SwitchCard = React.forwardRef<HTMLDivElement, SwitchCardProps>(
  (
    { label, description, checked, onCheckedChange, className, disabled },
    ref,
  ) => {
    return (
      <FormItem
        ref={ref}
        className={cn(
          'flex flex-row items-center justify-between rounded-lg border p-3 shadow-sm',
          className,
        )}
      >
        <div className="space-y-0.5">
          <FormLabel className={disabled ? 'text-muted-foreground' : ''}>
            {label}
          </FormLabel>
          {description && (
            <p className="text-xs text-muted-foreground">{description}</p>
          )}
        </div>
        <FormControl>
          <Switch
            checked={checked}
            onCheckedChange={onCheckedChange}
            disabled={disabled}
          />
        </FormControl>
      </FormItem>
    );
  },
);
SwitchCard.displayName = 'SwitchCard';

export { SwitchCard };
