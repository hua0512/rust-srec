import React from 'react';
import { useWatch } from 'react-hook-form';
import type { MessageDescriptor } from '@lingui/core';
import { msg } from '@lingui/core/macro';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { Info, RotateCcw, type LucideIcon } from 'lucide-react';
import {
  Accordion,
  AccordionContent,
  AccordionItem,
  AccordionTrigger,
} from '@/components/ui/accordion';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { cn } from '@/lib/utils';
import {
  DisableableDurationField,
  KeyValueField,
  PolicyField,
  SettingGrid,
  SettingInput,
  SettingList,
  SettingSelect,
  SettingSwitch,
  useEffectiveValue,
  useEngineOverride,
  type PolicyOption,
  type SettingOption,
  type SettingText,
} from './setting-fields';

/*
 * Engine settings declared as data: each field's path, label and engine default, grouped into the
 * sections they are shown in. The same lists render the form and count, per section, the fields
 * that hold something other than their default. Paths are relative to the engine's settings.
 */

interface FieldBase {
  path: string;
  label: MessageDescriptor;
  description?: MessageDescriptor;
  /** A switch this field only matters while on; the field is disabled while it is off. */
  requires?: Dependency;
}

export interface Dependency {
  path: string;
  fallback: boolean;
}

export type FieldSpec =
  | (FieldBase & {
      kind: 'number';
      fallback: number;
      fallbackLabel?: SettingText;
    })
  | (FieldBase & {
      kind: 'text';
      fallback: string;
      fallbackLabel?: SettingText;
      placeholder?: SettingText;
    })
  /** A number of seconds. */
  | (FieldBase & { kind: 'duration'; fallback: number })
  | (FieldBase & { kind: 'switch'; fallback: boolean })
  | (FieldBase & {
      kind: 'select';
      fallback: string;
      options: readonly SettingOption[];
    })
  | (FieldBase & {
      kind: 'policy';
      fallback: Record<string, unknown>;
      options: readonly PolicyOption[];
    })
  | (FieldBase & { kind: 'pairs'; fallback: readonly never[] })
  | (FieldBase & {
      kind: 'list';
      fallback: readonly never[];
      placeholder?: SettingText;
    })
  | (FieldBase & {
      kind: 'disableable';
      fallback: number | null;
      seed: number;
    });

export interface SectionSpec {
  id: string;
  label: MessageDescriptor;
  description?: MessageDescriptor;
  icon: LucideIcon;
  fields: readonly FieldSpec[];
  /** A switch the whole section only matters while on. */
  requires?: Dependency;
  /** A rule across the section's fields, shown below them. */
  after?: React.ComponentType<{ basePath: string }>;
}

/**
 * The value at `path` below `config`. In an override, a group reset with `null` resets every
 * field below it, which reads as `null`; in an engine, a `null` group is unset.
 */
function valueAt(config: unknown, path: string, isOverride: boolean): unknown {
  let current = config;
  for (const segment of path.split('.')) {
    if (current === null) return isOverride ? null : undefined;
    if (typeof current !== 'object') return undefined;
    current = (current as Record<string, unknown>)[segment];
  }
  return current;
}

/** A value with numeric text as numbers, as the form holds numbers typed into inputs. */
function normalized(value: unknown): string {
  return JSON.stringify(value, (_, entry: unknown) =>
    typeof entry === 'string' &&
    entry.trim() !== '' &&
    Number.isFinite(Number(entry))
      ? Number(entry)
      : entry,
  );
}

/**
 * Whether a field holds something other than its default. In an override, any value the field
 * holds counts: an unset field inherits the engine's value, and a `null` resets it.
 */
export function isChanged(
  config: unknown,
  field: FieldSpec,
  isOverride: boolean,
): boolean {
  const value = valueAt(config, field.path, isOverride);
  if (isOverride) return value !== undefined;
  if (value === undefined || value === '') return false;
  return normalized(value) !== normalized(field.fallback);
}

export function changedCount(
  config: unknown,
  fields: readonly FieldSpec[],
  isOverride: boolean,
): number {
  return fields.filter((field) => isChanged(config, field, isOverride)).length;
}

const SECTION_HEADING =
  'text-xs font-semibold uppercase tracking-wider text-muted-foreground';

/** Row of an engine's on/off setting. An override's shows a select under its label instead. */
const SWITCH_ROW =
  'rounded-xl border border-border/40 bg-muted/5 px-3 py-2.5 shadow-none';

/** How many of `fields` hold something other than their default (an override: hold anything). */
function useChangedCount(basePath: string, fields: readonly FieldSpec[]) {
  const { isOverride } = useEngineOverride();
  const config = useWatch({ name: basePath }) as unknown;
  return changedCount(config, fields, isOverride);
}

/** `compact` shows only the count, for a tab strip that must fit a phone's width. */
export function ChangedBadge({
  basePath,
  fields,
  compact = false,
}: {
  basePath: string;
  fields: readonly FieldSpec[];
  compact?: boolean;
}) {
  const { i18n } = useLingui();
  const { isOverride } = useEngineOverride();
  const count = useChangedCount(basePath, fields);
  if (count === 0) return null;
  const text = isOverride
    ? i18n._(msg`${count} overridden`)
    : i18n._(msg`${count} changed`);
  return (
    <Badge
      variant="secondary"
      title={compact ? text : undefined}
      className="h-5 min-w-5 shrink-0 justify-center rounded-full px-1.5 text-[10px] font-medium tabular-nums"
    >
      {compact ? (
        <>
          <span aria-hidden="true">{count}</span>
          <span className="sr-only">{text}</span>
        </>
      ) : (
        text
      )}
    </Badge>
  );
}

/** Returns a section's settings to their defaults, or in an override to the engine's settings. */
function SectionReset({
  basePath,
  fields,
  className,
}: {
  basePath: string;
  fields: readonly FieldSpec[];
  className?: string;
}) {
  const { isOverride, setField } = useEngineOverride();
  const count = useChangedCount(basePath, fields);
  if (count === 0) return null;
  return (
    <Button
      type="button"
      variant="ghost"
      size="sm"
      className={cn(
        'h-7 shrink-0 px-2 text-xs text-muted-foreground',
        className,
      )}
      onClick={() => {
        for (const field of fields) {
          setField(`${basePath}.${field.path}`, undefined);
        }
      }}
    >
      <RotateCcw className="h-3.5 w-3.5" />
      {isOverride ? (
        <Trans>Use engine settings</Trans>
      ) : (
        <Trans>Restore defaults</Trans>
      )}
    </Button>
  );
}

function FieldControl({
  field,
  basePath,
}: {
  field: FieldSpec;
  basePath: string;
}) {
  const { i18n } = useLingui();
  const { isOverride } = useEngineOverride();
  const name = `${basePath}.${field.path}`;
  const label = i18n._(field.label);
  const description = field.description && i18n._(field.description);
  const text = (value: SettingText | undefined) =>
    value === undefined || typeof value === 'string' ? value : i18n._(value);

  switch (field.kind) {
    case 'number':
    case 'duration':
      return (
        <SettingInput
          name={name}
          label={label}
          description={description}
          fallback={field.fallback}
          fallbackLabel={
            field.kind === 'number' ? field.fallbackLabel : undefined
          }
          kind={field.kind}
        />
      );
    case 'text':
      return (
        <SettingInput
          name={name}
          label={label}
          description={description}
          fallback={field.fallback}
          fallbackLabel={field.fallbackLabel}
          placeholder={text(field.placeholder)}
          kind="text"
        />
      );
    case 'switch':
      return (
        <SettingSwitch
          name={name}
          label={label}
          description={description}
          fallback={field.fallback}
          className={isOverride ? undefined : SWITCH_ROW}
        />
      );
    case 'select':
      return (
        <SettingSelect
          name={name}
          label={label}
          description={description}
          options={field.options}
          fallback={field.fallback}
        />
      );
    case 'policy':
      return (
        <PolicyField
          name={name}
          label={label}
          description={description}
          options={field.options}
          fallback={field.fallback}
        />
      );
    case 'pairs':
      return (
        <KeyValueField name={name} label={label} description={description} />
      );
    case 'list':
      return (
        <SettingList
          name={name}
          label={label}
          description={description}
          placeholder={text(field.placeholder)}
        />
      );
    case 'disableable':
      return (
        <DisableableDurationField
          name={name}
          label={label}
          description={description}
          fallback={field.fallback}
          seed={field.seed}
        />
      );
  }
}

/**
 * Settings that only matter while another setting is on: shown, so a value they hold stays
 * visible, but disabled.
 */
function Requires({
  basePath,
  requires,
  children,
}: {
  basePath: string;
  requires: Dependency;
  children: React.ReactNode;
}) {
  const enabled = useEffectiveValue(
    `${basePath}.${requires.path}`,
    requires.fallback,
  );
  return (
    <fieldset
      disabled={!enabled}
      className={cn('min-w-0', !enabled && 'opacity-50')}
    >
      {children}
    </fieldset>
  );
}

type FieldGroup = {
  layout: 'cells' | 'switches' | 'wide';
  fields: FieldSpec[];
};

/** Consecutive fields that share a layout: inputs and selects, on/off rows, or full-width lists. */
function groupFields(fields: readonly FieldSpec[]): FieldGroup[] {
  const groups: FieldGroup[] = [];
  for (const field of fields) {
    const layout =
      field.kind === 'switch'
        ? 'switches'
        : field.kind === 'pairs' || field.kind === 'list'
          ? 'wide'
          : 'cells';
    const last = groups[groups.length - 1];
    if (last && last.layout === layout && layout !== 'wide') {
      last.fields.push(field);
    } else {
      groups.push({ layout, fields: [field] });
    }
  }
  return groups;
}

function SectionFields({
  section,
  basePath,
  disabled,
}: {
  section: SectionSpec;
  basePath: string;
  disabled: boolean;
}) {
  const After = section.after;
  const body = (
    <div className="space-y-4">
      {groupFields(section.fields).map((group) => {
        const controls = group.fields.map((field) =>
          field.requires ? (
            <Requires
              key={field.path}
              basePath={basePath}
              requires={field.requires}
            >
              <FieldControl field={field} basePath={basePath} />
            </Requires>
          ) : (
            <FieldControl key={field.path} field={field} basePath={basePath} />
          ),
        );
        return group.layout === 'wide' ? (
          <React.Fragment key={group.fields[0].path}>{controls}</React.Fragment>
        ) : (
          <SettingGrid key={group.fields[0].path}>{controls}</SettingGrid>
        );
      })}
      {After && <After basePath={basePath} />}
    </div>
  );

  return (
    <fieldset
      disabled={disabled}
      className={cn('min-w-0', disabled && 'opacity-50')}
    >
      {section.requires ? (
        <Requires basePath={basePath} requires={section.requires}>
          {body}
        </Requires>
      ) : (
        body
      )}
    </fieldset>
  );
}

/** A section's description, or `notice` in its place when one explains why it is disabled. */
function SectionNote({
  section,
  notice,
}: {
  section: SectionSpec;
  notice?: React.ReactNode;
}) {
  const { i18n } = useLingui();
  if (!section.description && !notice) return null;
  return (
    <p className="flex items-start gap-2 text-xs text-muted-foreground">
      {notice && <Info className="mt-px h-3.5 w-3.5 shrink-0" />}
      <span>
        {notice ?? (section.description && i18n._(section.description))}
      </span>
    </p>
  );
}

/** A prominent on/off setting that governs the settings after it, with anything it explains. */
export function FeatureSwitch({
  field,
  basePath,
  badge,
  children,
}: {
  field: FieldSpec & { kind: 'switch' };
  basePath: string;
  badge?: React.ReactNode;
  children?: React.ReactNode;
}) {
  const { i18n } = useLingui();
  const { isOverride } = useEngineOverride();
  return (
    <div
      className={cn(
        'space-y-3 rounded-xl border border-border/50 bg-muted/10 p-4',
        !isOverride && 'py-3',
      )}
    >
      <SettingSwitch
        name={`${basePath}.${field.path}`}
        label={i18n._(field.label)}
        description={field.description && i18n._(field.description)}
        fallback={field.fallback}
        badge={badge}
        size="default"
        switchClassName=""
      />
      {children}
    </div>
  );
}

/** A section shown in full under its heading. */
export function SettingsSection({
  section,
  basePath,
  disabled = false,
  notice,
}: {
  section: SectionSpec;
  basePath: string;
  disabled?: boolean;
  /** Why the section is disabled, shown in place of its description. */
  notice?: React.ReactNode;
}) {
  const { i18n } = useLingui();
  return (
    <section className="space-y-4">
      <div className="space-y-1">
        {/* Wraps the reset under the heading when a narrow card cannot fit both. */}
        <div className="flex min-h-7 flex-wrap items-center gap-x-3 gap-y-1">
          <div className="flex items-center gap-2">
            <h3 className={SECTION_HEADING}>{i18n._(section.label)}</h3>
            <ChangedBadge basePath={basePath} fields={section.fields} />
          </div>
          <SectionReset
            basePath={basePath}
            fields={section.fields}
            className="ml-auto"
          />
        </div>
        <SectionNote section={section} notice={notice} />
      </div>
      <SectionFields
        section={section}
        basePath={basePath}
        disabled={disabled}
      />
    </section>
  );
}

/** Sections folded away under an "Advanced" heading. */
export function AdvancedSections({
  sections,
  basePath,
  disabled,
  notice,
}: {
  sections: readonly SectionSpec[];
  basePath: string;
  disabled?: (section: SectionSpec) => boolean;
  /** Why a disabled section is disabled. */
  notice?: React.ReactNode;
}) {
  const { i18n } = useLingui();
  if (sections.length === 0) return null;
  return (
    <section className="space-y-1">
      <h3 className={SECTION_HEADING}>
        <Trans>Advanced</Trans>
      </h3>
      <Accordion type="multiple" className="w-full">
        {sections.map((section) => {
          const Icon = section.icon;
          const off = disabled?.(section) ?? false;
          return (
            <AccordionItem
              key={section.id}
              value={section.id}
              className="border-border/50"
            >
              <AccordionTrigger className="items-center py-3 hover:no-underline">
                <span className="flex min-w-0 flex-1 items-center gap-2.5">
                  <Icon className="h-4 w-4 shrink-0 text-muted-foreground" />
                  <span className="truncate text-sm font-medium">
                    {i18n._(section.label)}
                  </span>
                  <ChangedBadge basePath={basePath} fields={section.fields} />
                </span>
              </AccordionTrigger>
              <AccordionContent className="space-y-4 pb-5 pt-1">
                <div className="flex items-start justify-between gap-3 empty:hidden">
                  <SectionNote
                    section={section}
                    notice={off ? notice : undefined}
                  />
                  <SectionReset
                    basePath={basePath}
                    fields={section.fields}
                    className="ml-auto"
                  />
                </div>
                <SectionFields
                  section={section}
                  basePath={basePath}
                  disabled={off}
                />
              </AccordionContent>
            </AccordionItem>
          );
        })}
      </Accordion>
    </section>
  );
}
