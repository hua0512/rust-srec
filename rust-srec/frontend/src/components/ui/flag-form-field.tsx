import React from 'react';
import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
} from './form';
import { Badge } from './badge';
import { Switch } from './switch';

type FlagFormFieldProps = {
  controlPrefix?: string;
  fieldName: string;
  title?: string | React.ReactNode;
  description?: string | React.ReactNode;
  ariaLabel?: string;
  /**
   * Rendered beside the title. Takes a `FieldInfo` when the reason for the setting is longer
   * than the one line `description` has room for, so the row stays a row.
   */
  info?: React.ReactNode;
  checked?: (value: any) => boolean;
  onChange?: (value: boolean) => void;
  showExperimentalBadge?: boolean;
  children?: React.ReactNode;
};

export function FlagFormField({
  controlPrefix,
  fieldName,
  title,
  description,
  ariaLabel,
  info,
  checked,
  onChange,
  showExperimentalBadge,
  children,
}: FlagFormFieldProps) {
  return (
    <FormField
      name={controlPrefix ? `${controlPrefix}.${fieldName}` : fieldName}
      render={({ field }) => (
        <FormItem className="flex flex-row items-center justify-between gap-4 rounded-xl border border-border/50 bg-background/50 px-4 py-3 shadow-sm">
          <div className="min-w-0 space-y-1">
            <FormLabel>
              <div className={'flex flex-row items-center gap-x-2'}>
                {title}
                {info}
                {showExperimentalBadge && <Badge>Experimental</Badge>}
              </div>
            </FormLabel>
            {description && (
              <FormDescription className="text-xs leading-relaxed">
                {description}
              </FormDescription>
            )}
            {children}
          </div>
          <FormControl>
            <Switch
              className="shrink-0"
              checked={checked ? checked(field.value) : field.value}
              onCheckedChange={(value) => {
                field.onChange(value);
                if (onChange) {
                  onChange(value);
                }
              }}
              aria-label={ariaLabel}
            />
          </FormControl>
        </FormItem>
      )}
    />
  );
}
