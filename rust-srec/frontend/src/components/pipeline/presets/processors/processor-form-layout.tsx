import type { ComponentProps, ComponentType, ReactNode } from 'react';
import type { Control, FieldPath, FieldValues } from 'react-hook-form';
import { motion } from 'motion/react';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormMessage,
} from '@/components/ui/form';
import { Input } from '@/components/ui/input';
import { ListInput } from '@/components/ui/list-input';
import { NumberInput } from '@/components/ui/number-input';
import { TabsTrigger } from '@/components/ui/tabs';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import {
  CONFIG_DESCRIPTION,
  CONFIG_INPUT,
  CONFIG_SELECT_CONTENT,
  CONFIG_SELECT_TRIGGER,
  ConfigFieldLabel,
} from '@/components/config/shared/config-field';
import { cn } from '@/lib/utils';
import { TimeAnchorSchema, type TimeAnchor } from '../processor-schemas';

type IconComponent = ComponentType<{ className?: string }>;

const shellVariants = {
  hidden: { opacity: 0, y: 20 },
  visible: { opacity: 1, y: 0, transition: { duration: 0.3 } },
};

/** Fade-and-rise entrance shared by the processor config forms. */
export function ProcessorFormShell({ children }: { children: ReactNode }) {
  return (
    <motion.div
      variants={shellVariants}
      initial="hidden"
      animate="visible"
      className="w-full"
    >
      {children}
    </motion.div>
  );
}

/** Bordered group of fields under an icon heading. */
export function ProcessorSection({
  icon: Icon,
  iconClassName,
  title,
  actions,
  children,
}: {
  icon: IconComponent;
  /** Accent colour of the heading icon. */
  iconClassName?: string;
  title: ReactNode;
  /** Rendered at the trailing end of the heading row. */
  actions?: ReactNode;
  children: ReactNode;
}) {
  return (
    <div className="p-4 rounded-xl bg-muted/10 border border-border/40 space-y-4">
      <div className="flex items-center gap-2 pb-2 border-b border-border/40 mb-2">
        <Icon className={cn('w-4 h-4', iconClassName)} />
        <h3 className="font-semibold text-sm mr-auto">{title}</h3>
        {actions}
      </div>
      {children}
    </div>
  );
}

/** Icon tab of a processor form's `TabsList`; the label hides on narrow screens. */
export function ProcessorTabTrigger({
  value,
  icon: Icon,
  children,
}: {
  value: string;
  icon: IconComponent;
  children: ReactNode;
}) {
  return (
    <TabsTrigger
      value={value}
      className="flex-1 min-w-[100px] flex flex-col sm:flex-row items-center justify-center gap-1.5 sm:gap-2 h-auto py-2.5 rounded-lg data-[state=active]:bg-background data-[state=active]:text-primary data-[state=active]:shadow-sm transition-all duration-300 relative overflow-hidden group"
    >
      <Icon className="w-4 h-4 shrink-0 transition-transform duration-300 group-data-[state=active]:scale-110" />
      <span className="text-xs font-medium hidden sm:inline">{children}</span>
    </TabsTrigger>
  );
}

/**
 * Subtle subsection heading inside a `ProcessorSection`, grouping related
 * fields without another bordered box.
 */
export function ProcessorSubsectionHeader({
  icon: Icon,
  children,
}: {
  icon: IconComponent;
  children: ReactNode;
}) {
  return (
    <div className="flex items-center gap-2 pb-1">
      <Icon className="h-3.5 w-3.5 text-muted-foreground/70" />
      <h4 className="text-xs font-medium uppercase tracking-wider text-muted-foreground/80">
        {children}
      </h4>
    </div>
  );
}

interface ProcessorFieldProps {
  control: Control<FieldValues>;
  name: FieldPath<FieldValues>;
  label: ReactNode;
  /** A `FieldHint` shown beside the label. */
  hint?: ReactNode;
  description?: ReactNode;
}

/** Label and hint above a field; the hint stays outside the label so it is not read as part of the field's name. */
function ProcessorFieldLabel({
  label,
  hint,
}: Pick<ProcessorFieldProps, 'label' | 'hint'>) {
  if (!hint) return <ConfigFieldLabel>{label}</ConfigFieldLabel>;
  return (
    <div className="flex items-center gap-1.5">
      <ConfigFieldLabel>{label}</ConfigFieldLabel>
      {hint}
    </div>
  );
}

function ProcessorFieldDescription({ children }: { children?: ReactNode }) {
  return children ? (
    <FormDescription className={CONFIG_DESCRIPTION}>{children}</FormDescription>
  ) : null;
}

/** Text input bound to a config string; an emptied box stores `''`. */
export function ProcessorTextField({
  control,
  name,
  label,
  hint,
  description,
  className,
  ...inputProps
}: ProcessorFieldProps &
  Omit<
    ComponentProps<typeof Input>,
    'name' | 'value' | 'onChange' | 'onBlur' | 'ref'
  >) {
  return (
    <FormField
      control={control}
      name={name}
      render={({ field }) => (
        <FormItem>
          <ProcessorFieldLabel label={label} hint={hint} />
          <FormControl>
            <Input
              {...inputProps}
              {...field}
              value={field.value ?? ''}
              className={cn(CONFIG_INPUT, className)}
            />
          </FormControl>
          <ProcessorFieldDescription>{description}</ProcessorFieldDescription>
          <FormMessage />
        </FormItem>
      )}
    />
  );
}

/** Number input bound to a config number; an emptied box stores `undefined` (see `NumberInput`). */
export function ProcessorNumberField({
  control,
  name,
  label,
  hint,
  description,
  className,
  ...inputProps
}: ProcessorFieldProps & Omit<ComponentProps<typeof NumberInput>, 'field'>) {
  return (
    <FormField
      control={control}
      name={name}
      render={({ field }) => (
        <FormItem>
          <ProcessorFieldLabel label={label} hint={hint} />
          <FormControl>
            <NumberInput
              {...inputProps}
              field={field}
              className={cn(CONFIG_INPUT, className)}
            />
          </FormControl>
          <ProcessorFieldDescription>{description}</ProcessorFieldDescription>
          <FormMessage />
        </FormItem>
      )}
    />
  );
}

/** List of strings bound to a config array; `label` may be left out where the section title names it. */
export function ProcessorListField({
  control,
  name,
  label,
  description,
  placeholder,
}: Omit<ProcessorFieldProps, 'label' | 'hint'> & {
  label?: ReactNode;
  placeholder?: string;
}) {
  return (
    <FormField
      control={control}
      name={name}
      render={({ field }) => (
        <FormItem>
          {label && <ConfigFieldLabel>{label}</ConfigFieldLabel>}
          <FormControl>
            <ListInput
              value={field.value || []}
              onChange={field.onChange}
              placeholder={placeholder}
            />
          </FormControl>
          <ProcessorFieldDescription>{description}</ProcessorFieldDescription>
          <FormMessage />
        </FormItem>
      )}
    />
  );
}

interface ProcessorSelectOption {
  value: string;
  label: ReactNode;
}

/** Select bound to a config enum. */
export function ProcessorSelectField({
  control,
  name,
  label,
  hint,
  description,
  placeholder,
  options,
}: ProcessorFieldProps & {
  placeholder?: string;
  options: readonly ProcessorSelectOption[];
}) {
  return (
    <FormField
      control={control}
      name={name}
      render={({ field }) => (
        <FormItem>
          <ProcessorFieldLabel label={label} hint={hint} />
          <Select onValueChange={field.onChange} value={field.value ?? ''}>
            <FormControl>
              <SelectTrigger className={CONFIG_SELECT_TRIGGER}>
                <SelectValue placeholder={placeholder} />
              </SelectTrigger>
            </FormControl>
            <SelectContent className={CONFIG_SELECT_CONTENT}>
              {options.map((option) => (
                <SelectItem key={option.value} value={option.value}>
                  {option.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <ProcessorFieldDescription>{description}</ProcessorFieldDescription>
          <FormMessage />
        </FormItem>
      )}
    />
  );
}

const TIME_ANCHOR_LABELS: Record<TimeAnchor, ReactNode> = {
  job_created: <Trans>Job creation time</Trans>,
  session_start: <Trans>Stream start time</Trans>,
};

/** Stands for an unset `time_anchor`, which the copy/move processor resolves to the time it runs. */
const EXECUTION_TIME_ANCHOR = 'execution_time';

/**
 * Picks which time the date tokens of a destination template are taken from.
 *
 * With `executionTime`, the field is optional and unset means the time the step runs, offered
 * as an extra first option; otherwise unset falls back to the job creation time.
 */
export function TimeAnchorField({
  control,
  name,
  description,
  executionTime = false,
}: Pick<ProcessorFieldProps, 'control' | 'name' | 'description'> & {
  executionTime?: boolean;
}) {
  const { i18n } = useLingui();
  return (
    <FormField
      control={control}
      name={name}
      render={({ field }) => (
        <FormItem>
          <ConfigFieldLabel>
            <Trans>Date placeholder anchor</Trans>
          </ConfigFieldLabel>
          <Select
            onValueChange={(value) =>
              field.onChange(
                value === EXECUTION_TIME_ANCHOR ? undefined : value,
              )
            }
            value={field.value ?? (executionTime ? EXECUTION_TIME_ANCHOR : '')}
          >
            <FormControl>
              <SelectTrigger className={CONFIG_SELECT_TRIGGER}>
                <SelectValue placeholder={i18n._(msg`Select date anchor`)} />
              </SelectTrigger>
            </FormControl>
            <SelectContent className={CONFIG_SELECT_CONTENT}>
              {executionTime && (
                <SelectItem value={EXECUTION_TIME_ANCHOR}>
                  <Trans>Execution time (default)</Trans>
                </SelectItem>
              )}
              {TimeAnchorSchema.options.map((anchor) => (
                <SelectItem key={anchor} value={anchor}>
                  {anchor === 'job_created' && !executionTime ? (
                    <Trans>Job creation time (default)</Trans>
                  ) : (
                    TIME_ANCHOR_LABELS[anchor]
                  )}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <ProcessorFieldDescription>{description}</ProcessorFieldDescription>
          <FormMessage />
        </FormItem>
      )}
    />
  );
}
