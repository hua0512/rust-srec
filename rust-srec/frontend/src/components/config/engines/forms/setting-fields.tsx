import React from 'react';
import { useFormContext, useFormState, useWatch } from 'react-hook-form';
import type { I18n, MessageDescriptor } from '@lingui/core';
import { msg, plural } from '@lingui/core/macro';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { Plus, Trash2, type LucideIcon } from 'lucide-react';
import { z } from 'zod';
import { Button } from '@/components/ui/button';
import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormMessage,
} from '@/components/ui/form';
import { Input } from '@/components/ui/input';
import { Switch } from '@/components/ui/switch';
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
import { useDefaultPlaceholder } from '@/hooks/use-default-placeholder';
import { useRowKeys } from '@/hooks/use-row-keys';
import { cn } from '@/lib/utils';

/*
 * Fields for an engine's settings that also edit a template's override of that engine.
 *
 * In an override, an absent field inherits the engine's value and `null` resets it to the engine
 * default (the backend merge removes null-valued keys), so each field offers both choices beside
 * a custom value. A field registers its input only while it holds a custom value: a controller
 * mounted under a `null` group would take `null` from it and turn the group back into an object.
 */

/**
 * A grid of settings, as many across as the form's width fits rather than the window's: beside
 * the sidebar and the settings navigation a wide window can still leave a narrow form. Override
 * fields name the value they resolve to in a select, so they get wider columns.
 */
export function SettingGrid({
  className,
  children,
}: {
  className?: string;
  children: React.ReactNode;
}) {
  const isOverride = React.useContext(EngineOverrideContext) !== null;
  return (
    <div
      className={cn(
        'grid gap-4',
        isOverride
          ? 'grid-cols-[repeat(auto-fit,minmax(min(100%,16rem),1fr))]'
          : 'grid-cols-[repeat(auto-fit,minmax(min(100%,14rem),1fr))]',
        className,
      )}
    >
      {children}
    </div>
  );
}

/** An override select trigger: its value wraps rather than cutting off what it resolves to. */
export const OVERRIDE_SELECT_TRIGGER = cn(
  CONFIG_SELECT_TRIGGER,
  'h-auto min-h-11 whitespace-normal text-left *:data-[slot=select-value]:line-clamp-none',
);

/** Select values for the two non-value states of an override field. */
export const INHERIT = 'inherit';
export const RESET = 'reset';

export type OverrideMode = typeof INHERIT | typeof RESET | 'custom';

export function overrideMode(value: unknown): OverrideMode {
  if (value === undefined) return INHERIT;
  if (value === null) return RESET;
  return 'custom';
}

/** The choice that uses the default, labelled with what that is. */
function defaultChoice(i18n: I18n, fallback: string): string {
  return i18n._(msg`Default (${fallback})`);
}

/** The two non-value choices an override field offers, labelled with what they resolve to. */
export function OverrideStateItems({
  inherited,
  fallback,
}: {
  inherited: string;
  fallback: string;
}) {
  const { i18n } = useLingui();
  return (
    <>
      <SelectItem value={INHERIT}>
        {i18n._(msg`Use engine setting (${inherited})`)}
      </SelectItem>
      <SelectItem value={RESET}>{defaultChoice(i18n, fallback)}</SelectItem>
    </>
  );
}

/** A setting's value as a select option label. `null` is an engine's disabled timeout. */
export function describeValue(i18n: I18n, value: unknown): string {
  if (value === true) return i18n._(msg`On`);
  if (value === false) return i18n._(msg`Off`);
  if (value === null) return i18n._(msg`Disabled`);
  return typeof value === 'number' || typeof value === 'string'
    ? String(value)
    : JSON.stringify(value);
}

interface EngineOverrideContextValue {
  /** Form path of the override, such as `engines_override.<engine id>`. */
  basePath: string;
  /** The overridden engine's configuration, which unset override fields inherit. */
  engineConfig: Record<string, unknown>;
  /** The override's schema, which lists the fields of a group reset with `null`. */
  schema: z.ZodObject;
}

const EngineOverrideContext =
  React.createContext<EngineOverrideContextValue | null>(null);

/** Marks the fields below as editing a template's override of an engine. */
export function EngineOverrideProvider({
  basePath,
  engineConfig,
  schema,
  children,
}: EngineOverrideContextValue & { children: React.ReactNode }) {
  const value = React.useMemo(
    () => ({ basePath, engineConfig, schema }),
    [basePath, engineConfig, schema],
  );
  return (
    <EngineOverrideContext.Provider value={value}>
      {children}
    </EngineOverrideContext.Provider>
  );
}

function valueAt(source: unknown, segments: readonly string[]): unknown {
  let current = source;
  for (const segment of segments) {
    if (current === null || typeof current !== 'object') return undefined;
    current = (current as Record<string, unknown>)[segment];
  }
  return current;
}

/** The object fields of `schema` behind its optional, nullable and transform wrappers. */
function objectFields(
  schema: unknown,
): Record<string, z.core.$ZodType> | undefined {
  let current = schema;
  for (;;) {
    if (current instanceof z.ZodObject) return current.shape;
    if (current instanceof z.ZodOptional || current instanceof z.ZodNullable) {
      current = current.unwrap();
    } else if (current instanceof z.ZodPipe) {
      current = current.in;
    } else {
      return undefined;
    }
  }
}

/** Override path segments of a form path below the override's base path. */
function relativeSegments(basePath: string, path: string): string[] {
  return path.startsWith(`${basePath}.`)
    ? path.slice(basePath.length + 1).split('.')
    : [];
}

/**
 * Whether the fields edit a template override, the engine value an override field inherits, and
 * a setter for override fields.
 */
export function useEngineOverride() {
  const context = React.useContext(EngineOverrideContext);
  const { getValues, setValue } = useFormContext<any>();

  const inherited = React.useCallback(
    (path: string): unknown =>
      context
        ? valueAt(
            context.engineConfig,
            relativeSegments(context.basePath, path),
          )
        : undefined,
    [context],
  );

  /**
   * Set an override field. A group reset with `null` above the field is first replaced by an
   * object resetting each of its fields, so the other fields of the group stay reset.
   */
  const setField = React.useCallback(
    (path: string, value: unknown) => {
      if (context) {
        const segments = relativeSegments(context.basePath, path);
        let fields = objectFields(context.schema);
        for (let depth = 0; depth < segments.length - 1; depth += 1) {
          const groupSegments = segments.slice(0, depth + 1);
          fields = objectFields(fields?.[segments[depth]]);
          const groupPath = [context.basePath, ...groupSegments].join('.');
          if (getValues(groupPath) !== null) continue;
          // Reset every field the form knows and every one the engine sets: a key left out
          // would inherit the engine's value instead.
          const engineGroup = valueAt(context.engineConfig, groupSegments);
          const keys = new Set([
            ...Object.keys(fields ?? {}),
            ...(engineGroup !== null && typeof engineGroup === 'object'
              ? Object.keys(engineGroup)
              : []),
          ]);
          setValue(
            groupPath,
            Object.fromEntries([...keys].map((key) => [key, null])),
            { shouldDirty: true },
          );
        }
      }
      setValue(path, value, { shouldDirty: true, shouldValidate: true });
    },
    [context, getValues, setValue],
  );

  return { isOverride: context !== null, inherited, setField };
}

/**
 * A field's form value. In an override, a field below a group reset with `null` reads as `null`:
 * the form itself reports such a field as absent, which would show it as inherited.
 */
export function useSettingValue(path: string): unknown {
  const context = React.useContext(EngineOverrideContext);
  // The field's groups, outermost first, then the field itself.
  const names = React.useMemo(() => {
    if (!context) return [path];
    const segments = relativeSegments(context.basePath, path);
    return segments.map((_, depth) =>
      [context.basePath, ...segments.slice(0, depth + 1)].join('.'),
    );
  }, [context, path]);
  const values = useWatch({ name: names }) as unknown[];
  return values.slice(0, -1).some((group) => group === null)
    ? null
    : values[values.length - 1];
}

/**
 * The value a setting takes effect with: in an override, an absent field takes the engine's value
 * and `null` the default.
 */
export function useEffectiveValue<T>(path: string, fallback: T): T {
  const { isOverride, inherited } = useEngineOverride();
  const value = useSettingValue(path) as T | null | undefined;
  if (value === null) return fallback;
  if (value !== undefined) return value;
  if (!isOverride) return fallback;
  return (inherited(path) as T | null | undefined) ?? fallback;
}

/** Label of an override field, which has no `FormItem` to label it. */
function OverrideLabel({
  labelId,
  label,
  icon,
  size,
}: {
  labelId: string;
  label: React.ReactNode;
  icon?: LucideIcon;
  size?: 'sm' | 'default';
}) {
  return (
    <ConfigFieldLabel plain icon={icon} size={size}>
      <span id={labelId}>{label}</span>
    </ConfigFieldLabel>
  );
}

/** Description of an override field, below its control as in an engine's fields. */
function OverrideDescription({ children }: { children?: React.ReactNode }) {
  return children ? <p className={CONFIG_DESCRIPTION}>{children}</p> : null;
}

/** A message, or a literal such as an identifier that is not translated. */
export type SettingText = MessageDescriptor | string;

function translate(i18n: I18n, text: SettingText): string {
  return typeof text === 'string' ? text : i18n._(text);
}

interface SettingInputProps {
  name: string;
  label: React.ReactNode;
  description?: React.ReactNode;
  /** The engine default, used when the field is unset. */
  fallback: number | string;
  /** How the default is shown when its raw value would not explain it. */
  fallbackLabel?: SettingText;
  /** Placeholder of an engine's empty input, in place of naming the default. */
  placeholder?: string;
  numeric?: boolean;
  size?: 'sm' | 'default';
}

/**
 * A number or text setting. In an engine, an empty input uses the default; in an override, a
 * select chooses between the engine's setting, the default and a custom value.
 */
export function SettingInput({
  name,
  label,
  description,
  fallback,
  fallbackLabel,
  placeholder,
  numeric = true,
  size,
}: SettingInputProps) {
  const { i18n } = useLingui();
  const labelId = React.useId();
  const defaultPlaceholder = useDefaultPlaceholder();
  const { isOverride, inherited, setField } = useEngineOverride();
  const value = useSettingValue(name);
  const shownDefault =
    fallbackLabel === undefined
      ? String(fallback)
      : translate(i18n, fallbackLabel);

  if (!isOverride) {
    return (
      <FormField
        name={name}
        render={({ field }) => (
          <FormItem className="space-y-2">
            <ConfigFieldLabel size={size}>{label}</ConfigFieldLabel>
            <FormControl>
              <Input
                type={numeric ? 'number' : 'text'}
                {...field}
                value={field.value ?? ''}
                className={cn(CONFIG_INPUT, 'font-mono')}
                placeholder={placeholder ?? defaultPlaceholder(shownDefault)}
              />
            </FormControl>
            {description && (
              <FormDescription className={CONFIG_DESCRIPTION}>
                {description}
              </FormDescription>
            )}
            <FormMessage />
          </FormItem>
        )}
      />
    );
  }

  const engineValue = inherited(name);
  const mode = overrideMode(value);
  return (
    <div className="space-y-2">
      <OverrideLabel labelId={labelId} label={label} size={size} />
      <Select
        value={mode}
        onValueChange={(next) =>
          setField(
            name,
            next === INHERIT
              ? undefined
              : next === RESET
                ? null
                : (engineValue ?? fallback),
          )
        }
      >
        <SelectTrigger
          aria-labelledby={labelId}
          className={OVERRIDE_SELECT_TRIGGER}
        >
          <SelectValue />
        </SelectTrigger>
        <SelectContent className={CONFIG_SELECT_CONTENT}>
          <OverrideStateItems
            inherited={
              engineValue == null
                ? shownDefault
                : describeValue(i18n, engineValue)
            }
            fallback={shownDefault}
          />
          <SelectItem value="custom">
            <Trans>Custom</Trans>
          </SelectItem>
        </SelectContent>
      </Select>
      {mode === 'custom' && (
        <FormField
          name={name}
          render={({ field }) => (
            <FormItem className="space-y-2">
              <FormControl>
                <Input
                  type={numeric ? 'number' : 'text'}
                  {...field}
                  value={field.value ?? ''}
                  aria-labelledby={labelId}
                  className={cn(CONFIG_INPUT, 'font-mono')}
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
      )}
      <OverrideDescription>{description}</OverrideDescription>
    </div>
  );
}

interface SettingSwitchProps {
  name: string;
  label: React.ReactNode;
  description?: React.ReactNode;
  /** The engine default, used when the field is unset. */
  fallback: boolean;
  icon?: LucideIcon;
  size?: 'sm' | 'default';
  /** Classes of the row holding the label and the control. */
  className?: string;
  switchClassName?: string;
}

/** An on/off setting: a switch in an engine, a select with the override states in an override. */
export function SettingSwitch({
  name,
  label,
  description,
  fallback,
  icon,
  size = 'sm',
  className,
  switchClassName = 'scale-75 origin-right',
}: SettingSwitchProps) {
  const { i18n } = useLingui();
  const labelId = React.useId();
  const defaultPlaceholder = useDefaultPlaceholder();
  const { isOverride, inherited, setField } = useEngineOverride();
  const value = useSettingValue(name) as boolean | null | undefined;

  if (!isOverride) {
    // A switch has no empty state to name the default in, so it names it once changed.
    const changed = typeof value === 'boolean' && value !== fallback;
    return (
      <FormField
        name={name}
        render={({ field }) => (
          <FormItem
            className={cn(
              'flex flex-row items-center justify-between gap-3 space-y-0',
              className,
            )}
          >
            <div className="min-w-0 space-y-0.5">
              <ConfigFieldLabel icon={icon} size={size}>
                {label}
              </ConfigFieldLabel>
              {(description || changed) && (
                <FormDescription className={CONFIG_DESCRIPTION}>
                  {description}
                  {description && changed && ' '}
                  {changed && defaultPlaceholder(describeValue(i18n, fallback))}
                </FormDescription>
              )}
            </div>
            <FormControl>
              <Switch
                checked={field.value ?? fallback}
                onCheckedChange={field.onChange}
                className={switchClassName}
              />
            </FormControl>
          </FormItem>
        )}
      />
    );
  }

  const engineValue = inherited(name);
  return (
    // Label above the select, like the other override fields, so a row of them lines up.
    <div className={cn('space-y-2', className)}>
      <OverrideLabel labelId={labelId} label={label} icon={icon} size={size} />
      <Select
        value={typeof value === 'boolean' ? String(value) : overrideMode(value)}
        onValueChange={(next) =>
          setField(
            name,
            next === INHERIT
              ? undefined
              : next === RESET
                ? null
                : next === 'true',
          )
        }
      >
        <SelectTrigger
          aria-labelledby={labelId}
          className={OVERRIDE_SELECT_TRIGGER}
        >
          <SelectValue />
        </SelectTrigger>
        <SelectContent className={CONFIG_SELECT_CONTENT}>
          <OverrideStateItems
            inherited={describeValue(
              i18n,
              typeof engineValue === 'boolean' ? engineValue : fallback,
            )}
            fallback={describeValue(i18n, fallback)}
          />
          <SelectItem value="true">{i18n._(msg`On`)}</SelectItem>
          <SelectItem value="false">{i18n._(msg`Off`)}</SelectItem>
        </SelectContent>
      </Select>
      <OverrideDescription>{description}</OverrideDescription>
    </div>
  );
}

export interface SettingOption {
  value: string;
  label: SettingText;
}

interface SettingSelectProps {
  name: string;
  label: React.ReactNode;
  description?: React.ReactNode;
  options: readonly SettingOption[];
  /** The option in effect when the field is unset. */
  fallback: string;
  icon?: LucideIcon;
}

/** A choice between named values. In an override, the list starts with the override states. */
export function SettingSelect({
  name,
  label,
  description,
  options,
  fallback,
  icon,
}: SettingSelectProps) {
  const { i18n } = useLingui();
  const labelId = React.useId();
  const { isOverride, inherited, setField } = useEngineOverride();
  const value = useSettingValue(name) as string | null | undefined;
  const describe = (candidate: unknown) => {
    const option = options.find((entry) => entry.value === candidate);
    return option
      ? translate(i18n, option.label)
      : describeValue(i18n, candidate);
  };
  const engineValue = inherited(name);

  const selected =
    typeof value === 'string' && value !== ''
      ? value
      : isOverride
        ? overrideMode(value)
        : fallback;

  return (
    <div className="space-y-2">
      <ConfigFieldLabel plain icon={icon}>
        <span id={labelId}>{label}</span>
      </ConfigFieldLabel>
      <Select
        value={selected}
        onValueChange={(next) =>
          setField(
            name,
            next === INHERIT ? undefined : next === RESET ? null : next,
          )
        }
      >
        <SelectTrigger
          aria-labelledby={labelId}
          className={CONFIG_SELECT_TRIGGER}
        >
          <SelectValue />
        </SelectTrigger>
        <SelectContent className={CONFIG_SELECT_CONTENT}>
          {isOverride && (
            <OverrideStateItems
              inherited={describe(
                typeof engineValue === 'string' ? engineValue : fallback,
              )}
              fallback={describe(fallback)}
            />
          )}
          {options.map((option) => (
            <SelectItem key={option.value} value={option.value}>
              {translate(i18n, option.label)}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
      <OverrideDescription>{description}</OverrideDescription>
    </div>
  );
}

/** The selects of the policy, list and timeout fields below, which name what each choice means. */
function StateSelect({
  labelId,
  value,
  onValueChange,
  children,
}: {
  labelId: string;
  value: string;
  onValueChange: (next: string) => void;
  children: React.ReactNode;
}) {
  return (
    <Select value={value} onValueChange={onValueChange}>
      <SelectTrigger
        aria-labelledby={labelId}
        className={OVERRIDE_SELECT_TRIGGER}
      >
        <SelectValue />
      </SelectTrigger>
      <SelectContent className={CONFIG_SELECT_CONTENT}>
        {children}
      </SelectContent>
    </Select>
  );
}

/** A value of a policy's payload, as option text. */
export interface PolicyPayload {
  key: string;
  label: MessageDescriptor;
  /** Unit after the label and the value, such as `ms`. */
  unit?: string;
  description?: MessageDescriptor;
  numeric?: boolean;
}

export interface PolicyOption {
  type: string;
  label: MessageDescriptor;
  /** The value written when this option is chosen. */
  value: Record<string, unknown>;
  /** The values the option carries beside its `type`, in the order they are described. */
  payload?: readonly PolicyPayload[];
  /** Joins the payload values when the policy is described; `, ` by default. */
  separator?: string;
}

/** A policy payload value as text, or `undefined` when it is not a plain value. */
function scalar(value: unknown): string | undefined {
  if (typeof value === 'number') return String(value);
  return typeof value === 'string' && value !== '' ? value : undefined;
}

export function describePolicy(
  i18n: I18n,
  options: readonly PolicyOption[],
  policy: unknown,
): string {
  if (!policy || typeof policy !== 'object') return describeValue(i18n, policy);
  const fields = policy as Record<string, unknown>;
  const option = options.find((candidate) => candidate.type === fields.type);
  if (!option) return scalar(fields.type) ?? '';
  const label = i18n._(option.label);
  const details = (option.payload ?? []).flatMap((payload) => {
    const value = scalar(fields[payload.key]);
    if (value === undefined) return [];
    return [payload.unit ? `${value} ${payload.unit}` : value];
  });
  return details.length > 0
    ? `${label} (${details.join(option.separator ?? ', ')})`
    : label;
}

function PayloadInput({
  name,
  payload,
}: {
  name: string;
  payload: PolicyPayload;
}) {
  const { i18n } = useLingui();
  const label = i18n._(payload.label);
  return (
    <FormField
      name={name}
      render={({ field }) => (
        <FormItem className="space-y-2">
          <ConfigFieldLabel size="sm">
            {payload.unit ? `${label} (${payload.unit})` : label}
          </ConfigFieldLabel>
          <FormControl>
            <Input
              type={payload.numeric === false ? 'text' : 'number'}
              {...field}
              value={field.value ?? ''}
              className={cn(CONFIG_INPUT, 'font-mono')}
            />
          </FormControl>
          {payload.description && (
            <FormDescription className={CONFIG_DESCRIPTION}>
              {i18n._(payload.description)}
            </FormDescription>
          )}
          <FormMessage />
        </FormItem>
      )}
    />
  );
}

/**
 * A choice of tagged policy object (`{ type, ...payload }`) whose payload values are edited below
 * it. In an engine, "Default" leaves the field unset; in an override, the field can also inherit
 * the engine's choice.
 */
export function PolicyField({
  name,
  label,
  description,
  options,
  fallback,
}: {
  name: string;
  label: React.ReactNode;
  description?: React.ReactNode;
  options: readonly PolicyOption[];
  /** The policy in effect when the field is unset. */
  fallback: Record<string, unknown>;
}) {
  const { i18n } = useLingui();
  const labelId = React.useId();
  const { isOverride, inherited, setField } = useEngineOverride();
  const value = useSettingValue(name) as { type?: string } | null | undefined;
  const isPolicy = value !== null && typeof value === 'object';
  const selected = isPolicy
    ? (value.type ?? RESET)
    : isOverride
      ? overrideMode(value)
      : RESET;
  const payload =
    options.find((option) => option.type === selected)?.payload ?? [];

  const select = (next: string) => {
    const option = options.find((candidate) => candidate.type === next);
    if (option) setField(name, { ...option.value });
    else if (next === RESET && isOverride) setField(name, null);
    else setField(name, undefined);
  };

  return (
    <div className="space-y-2">
      <OverrideLabel labelId={labelId} label={label} />
      <StateSelect labelId={labelId} value={selected} onValueChange={select}>
        {isOverride ? (
          <OverrideStateItems
            inherited={describePolicy(
              i18n,
              options,
              inherited(name) ?? fallback,
            )}
            fallback={describePolicy(i18n, options, fallback)}
          />
        ) : (
          <SelectItem value={RESET}>
            {defaultChoice(i18n, describePolicy(i18n, options, fallback))}
          </SelectItem>
        )}
        {options.map((option) => (
          <SelectItem key={option.type} value={option.type}>
            {i18n._(option.label)}
          </SelectItem>
        ))}
      </StateSelect>
      {isPolicy && payload.length > 0 && (
        <div className="grid gap-3 grid-cols-[repeat(auto-fit,minmax(min(100%,10rem),1fr))]">
          {payload.map((entry) => (
            <PayloadInput
              key={entry.key}
              name={`${name}.${entry.key}`}
              payload={entry}
            />
          ))}
        </div>
      )}
      <OverrideDescription>{description}</OverrideDescription>
    </div>
  );
}

function describePairs(i18n: I18n, pairs: unknown): string {
  const count = Array.isArray(pairs) ? pairs.length : 0;
  return count === 0
    ? i18n._(msg`None`)
    : i18n._(
        msg`${plural(count, { one: '# parameter', other: '# parameters' })}`,
      );
}

/** A list of key and value pairs, such as query parameters. */
export function KeyValueField({
  name,
  label,
  description,
}: {
  name: string;
  label: React.ReactNode;
  description?: React.ReactNode;
}) {
  const { i18n } = useLingui();
  const labelId = React.useId();
  const { isOverride, inherited, setField } = useEngineOverride();
  const raw = useSettingValue(name) as
    | Array<[string, string]>
    | null
    | undefined;
  const value = raw ?? [];
  const rowKeys = useRowKeys(value.length);
  const mode = overrideMode(raw);

  const updateEntry = (idx: number, next: [string, string]) => {
    setField(
      name,
      value.map((pair, i) => (i === idx ? next : pair)),
    );
  };

  const removeEntry = (idx: number) => {
    rowKeys.removeAt(idx);
    const nextValue = value.filter((_, i) => i !== idx);
    // In an override, an empty list replaces the engine's parameters with none.
    setField(name, nextValue.length > 0 || isOverride ? nextValue : undefined);
  };

  const editor = (
    <div className="space-y-2">
      {value.map(([k, v], idx) => (
        <div
          key={rowKeys.keyAt(idx)}
          className="grid grid-cols-[minmax(0,1fr)_minmax(0,1fr)_auto] gap-2"
        >
          <Input
            value={k}
            aria-label={i18n._(msg`Name`)}
            onChange={(e) => updateEntry(idx, [e.target.value, v])}
            className={cn(CONFIG_INPUT, 'font-mono')}
            placeholder="key"
          />
          <Input
            value={v}
            aria-label={i18n._(msg`Value`)}
            onChange={(e) => updateEntry(idx, [k, e.target.value])}
            className={cn(CONFIG_INPUT, 'font-mono')}
            placeholder="value"
          />
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="h-11 w-11 text-muted-foreground hover:text-destructive"
            onClick={() => removeEntry(idx)}
          >
            <Trash2 className="h-4 w-4" />
            <span className="sr-only">
              <Trans>Remove</Trans>
            </span>
          </Button>
        </div>
      ))}
      <Button
        type="button"
        variant="outline"
        size="sm"
        onClick={() => setField(name, [...value, ['', '']])}
      >
        <Plus className="h-4 w-4" />
        <Trans>Add parameter</Trans>
      </Button>
    </div>
  );

  if (!isOverride) {
    return (
      <div className="space-y-2">
        <OverrideLabel labelId={labelId} label={label} />
        {editor}
        <OverrideDescription>{description}</OverrideDescription>
      </div>
    );
  }

  const engineValue = inherited(name);
  return (
    <div className="space-y-2">
      <OverrideLabel labelId={labelId} label={label} />
      <StateSelect
        labelId={labelId}
        value={mode}
        onValueChange={(next) =>
          setField(
            name,
            next === INHERIT
              ? undefined
              : next === RESET
                ? null
                : Array.isArray(engineValue)
                  ? engineValue.map((pair) => [...pair])
                  : [],
          )
        }
      >
        <OverrideStateItems
          inherited={describePairs(i18n, engineValue)}
          fallback={describePairs(i18n, [])}
        />
        <SelectItem value="custom">
          <Trans>Custom</Trans>
        </SelectItem>
      </StateSelect>
      {mode === 'custom' && editor}
      <OverrideDescription>{description}</OverrideDescription>
    </div>
  );
}

/**
 * A timeout an engine can also disable, which it stores as `null`. A template override cannot:
 * the backend merge reads `null` there as "use the default", so an override offers the usual
 * override states instead.
 */
export function DisableableDurationField({
  name,
  label,
  description,
  fallback,
  seed,
}: {
  name: string;
  label: React.ReactNode;
  description?: React.ReactNode;
  /** The engine default: a duration in ms, or `null` when it is disabled by default. */
  fallback: number | null;
  /** The value a custom duration starts from. */
  seed: number;
}) {
  const { i18n } = useLingui();
  const labelId = React.useId();
  const { isOverride, inherited, setField } = useEngineOverride();
  const raw = useSettingValue(name) as unknown;
  // An emptied input holds `''`, which is unset, not disabled.
  const value = raw === '' ? undefined : raw;
  const custom = value !== undefined && value !== null;

  let control: React.ReactNode;
  if (isOverride) {
    const engineValue = inherited(name);
    control = (
      <StateSelect
        labelId={labelId}
        value={overrideMode(value)}
        onValueChange={(next) =>
          setField(
            name,
            next === INHERIT
              ? undefined
              : next === RESET
                ? null
                : typeof engineValue === 'number'
                  ? engineValue
                  : seed,
          )
        }
      >
        <OverrideStateItems
          inherited={describeValue(
            i18n,
            engineValue === undefined ? fallback : engineValue,
          )}
          fallback={describeValue(i18n, fallback)}
        />
        <SelectItem value="custom">
          <Trans>Custom</Trans>
        </SelectItem>
      </StateSelect>
    );
  } else {
    control = (
      <StateSelect
        labelId={labelId}
        value={value === null ? 'disabled' : custom ? 'custom' : 'default'}
        onValueChange={(next) => {
          if (next === 'default') setField(name, undefined);
          else if (next === 'disabled') setField(name, null);
          else if (!custom) setField(name, seed);
        }}
      >
        <SelectItem value="default">
          {defaultChoice(i18n, describeValue(i18n, fallback))}
        </SelectItem>
        <SelectItem value="disabled">
          <Trans>Disabled</Trans>
        </SelectItem>
        <SelectItem value="custom">
          <Trans>Custom</Trans>
        </SelectItem>
      </StateSelect>
    );
  }

  return (
    <div className="space-y-2">
      <OverrideLabel labelId={labelId} label={label} />
      {control}
      {custom && (
        <Input
          type="number"
          value={
            typeof value === 'number' || typeof value === 'string' ? value : ''
          }
          aria-labelledby={labelId}
          onChange={(e) =>
            setField(name, e.target.value === '' ? undefined : e.target.value)
          }
          className={cn(CONFIG_INPUT, 'font-mono')}
          placeholder={String(seed)}
        />
      )}
      <OverrideDescription>
        {description}
        {isOverride && (
          <>
            {description && ' '}
            <Trans>
              A template can't turn this off; do that on the engine.
            </Trans>
          </>
        )}
      </OverrideDescription>
    </div>
  );
}

/** The validation message of a path that no registered field shows, such as a rule across fields. */
export function FieldError({ name }: { name: string }) {
  const { control, getFieldState } = useFormContext();
  const formState = useFormState({ control, name });
  const message = getFieldState(name, formState).error?.message;
  return message ? (
    <p role="alert" className="text-destructive text-sm">
      {message}
    </p>
  ) : null;
}
