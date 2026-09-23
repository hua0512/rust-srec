import type { ReactNode } from 'react';
import type { FieldValues, Path, UseFormReturn } from 'react-hook-form';
import type { LucideIcon } from 'lucide-react';
import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
} from '@/components/ui/form';
import { Input } from '@/components/ui/input';
import {
  ConfigFieldLabel,
  ConfigSectionHeading,
  CONFIG_DESCRIPTION,
} from '@/components/config/shared/config-field';
import { configPath } from '@/components/config/shared/form-path';

interface SecretFieldSectionProps<TFieldValues extends FieldValues> {
  form: UseFormReturn<TFieldValues>;
  /** Path to the object holding this platform's options. */
  fieldName: Path<TFieldValues>;
  /** Key of the secret inside that object. */
  optionKey: string;
  icon: LucideIcon;
  heading: ReactNode;
  label: ReactNode;
  description: ReactNode;
  placeholder: string;
  autoComplete?: string;
}

/** A platform tab whose only option is one masked secret, such as a token or password. */
export function SecretFieldSection<TFieldValues extends FieldValues>({
  form,
  fieldName,
  optionKey,
  icon,
  heading,
  label,
  description,
  placeholder,
  autoComplete,
}: SecretFieldSectionProps<TFieldValues>) {
  return (
    <div className="space-y-12">
      <section className="space-y-6">
        <ConfigSectionHeading icon={icon} accent="indigo">
          {heading}
        </ConfigSectionHeading>

        <div className="grid gap-6">
          <FormField
            control={form.control}
            name={configPath<TFieldValues>(fieldName, optionKey)}
            render={({ field }) => (
              <FormItem className="space-y-4">
                <ConfigFieldLabel accent="indigo">{label}</ConfigFieldLabel>
                <FormControl>
                  <Input
                    type="password"
                    autoComplete={autoComplete}
                    {...field}
                    value={field.value || ''}
                    className="bg-background/50 h-10 rounded-xl border-border/50 focus:bg-background transition-all font-mono text-xs shadow-sm"
                    placeholder={placeholder}
                  />
                </FormControl>
                <FormDescription className={CONFIG_DESCRIPTION}>
                  {description}
                </FormDescription>
              </FormItem>
            )}
          />
        </div>
      </section>
    </div>
  );
}
