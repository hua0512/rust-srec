import React from 'react';
import { useFormContext, useWatch } from 'react-hook-form';
import { msg } from '@lingui/core/macro';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { Info, RotateCcw } from 'lucide-react';
import {
  Accordion,
  AccordionContent,
  AccordionItem,
  AccordionTrigger,
} from '@/components/ui/accordion';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';
import {
  MesioConfigOverrideSchema,
  type MesioConfig,
} from '@/api/schemas/engine';
import { cn } from '@/lib/utils';
import {
  DisableableDurationField,
  EngineOverrideProvider,
  FieldError,
  KeyValueField,
  PolicyField,
  SettingGrid,
  SettingInput,
  SettingSelect,
  SettingSwitch,
  useEffectiveValue,
  useEngineOverride,
} from './setting-fields';
import {
  MESIO_PROTOCOLS,
  changedCount,
  protocolFields,
  type Dependency,
  type FieldSpec,
  type ProtocolSpec,
  type SectionSpec,
} from './mesio-sections';

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
function ChangedBadge({
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

  switch (field.kind) {
    case 'number':
      return (
        <SettingInput
          name={name}
          label={label}
          description={description}
          fallback={field.fallback}
          fallbackLabel={field.fallbackLabel}
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
          placeholder={field.placeholder && i18n._(field.placeholder)}
          numeric={false}
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
        : field.kind === 'pairs'
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

/**
 * The validation message of the DASH track switches, which may not both be off. The rule spans two
 * fields, so it is checked again whenever either changes.
 */
function DashTrackCheck({ basePath }: { basePath: string }) {
  const { control, trigger } = useFormContext();
  const errorPath = `${basePath}.dash.include_audio`;
  const [video, audio] = useWatch({
    control,
    name: [`${basePath}.dash.include_video`, errorPath],
  }) as unknown[];
  const mounted = React.useRef(false);
  React.useEffect(() => {
    if (!mounted.current) {
      mounted.current = true;
      return;
    }
    void trigger(errorPath);
  }, [video, audio, errorPath, trigger]);
  return <FieldError name={errorPath} />;
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
      {section.check === 'dash-tracks' && (
        <DashTrackCheck basePath={basePath} />
      )}
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

/** A section's description and the notes on why its settings have no effect. */
function SectionNote({
  section,
  protocol,
  fixing,
}: {
  section: SectionSpec;
  protocol: ProtocolSpec;
  fixing: boolean;
}) {
  const { i18n } = useLingui();
  const name = protocol.name;
  const fixOff = Boolean(section.requiresFix) && !fixing;
  if (!section.description && !fixOff) return null;
  return (
    <p className="flex items-start gap-2 text-xs text-muted-foreground">
      {fixOff && <Info className="mt-px h-3.5 w-3.5 shrink-0" />}
      <span>
        {fixOff ? (
          <Trans>{name} fixing is off, so these settings have no effect.</Trans>
        ) : (
          section.description && i18n._(section.description)
        )}
      </span>
    </p>
  );
}

const ProtocolPanel = React.memo(function ProtocolPanel({
  protocol,
  basePath,
}: {
  protocol: ProtocolSpec;
  basePath: string;
}) {
  const { i18n } = useLingui();
  const { isOverride } = useEngineOverride();
  const fixing = useEffectiveValue(
    `${basePath}.${protocol.fix.path}`,
    protocol.fix.fallback,
  );
  const sectionDisabled = (section: SectionSpec) =>
    Boolean(section.requiresFix) && !fixing;
  const { essentials } = protocol;

  return (
    <div className="space-y-6">
      <div
        className={cn(
          'rounded-xl border border-border/50 bg-muted/10 p-4',
          !isOverride && 'py-3',
        )}
      >
        <SettingSwitch
          name={`${basePath}.${protocol.fix.path}`}
          label={i18n._(protocol.fix.label)}
          description={
            protocol.fix.description && i18n._(protocol.fix.description)
          }
          fallback={protocol.fix.fallback}
          size="default"
          switchClassName=""
        />
      </div>

      <section className="space-y-4">
        <div className="space-y-1">
          <div className="flex min-h-7 items-center justify-between gap-3">
            <h3 className={SECTION_HEADING}>{i18n._(essentials.label)}</h3>
            <SectionReset basePath={basePath} fields={essentials.fields} />
          </div>
          <SectionNote
            section={essentials}
            protocol={protocol}
            fixing={fixing}
          />
        </div>
        <SectionFields
          section={essentials}
          basePath={basePath}
          disabled={sectionDisabled(essentials)}
        />
      </section>

      {protocol.advanced.length > 0 && (
        <section className="space-y-1">
          <h3 className={SECTION_HEADING}>
            <Trans>Advanced</Trans>
          </h3>
          <Accordion type="multiple" className="w-full">
            {protocol.advanced.map((section) => {
              const Icon = section.icon;
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
                      <ChangedBadge
                        basePath={basePath}
                        fields={section.fields}
                      />
                    </span>
                  </AccordionTrigger>
                  <AccordionContent className="space-y-4 pb-5 pt-1">
                    <div className="flex items-start justify-between gap-3 empty:hidden">
                      <SectionNote
                        section={section}
                        protocol={protocol}
                        fixing={fixing}
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
                      disabled={sectionDisabled(section)}
                    />
                  </AccordionContent>
                </AccordionItem>
              );
            })}
          </Accordion>
        </section>
      )}
    </div>
  );
});

const PROTOCOL_FIELDS = new Map(
  MESIO_PROTOCOLS.map((protocol) => [protocol.id, protocolFields(protocol)]),
);

function MesioSettings({ basePath }: { basePath: string }) {
  return (
    <Tabs defaultValue="flv" className="w-full space-y-6">
      <TabsList className="grid h-auto w-full grid-cols-3 border border-border/40 bg-background/40 p-1">
        {MESIO_PROTOCOLS.map((protocol) => (
          <TabsTrigger
            key={protocol.id}
            value={protocol.id}
            className="min-w-0 gap-2 py-1.5"
          >
            {protocol.name}
            <ChangedBadge
              basePath={basePath}
              fields={PROTOCOL_FIELDS.get(protocol.id) ?? []}
              compact
            />
          </TabsTrigger>
        ))}
      </TabsList>
      {MESIO_PROTOCOLS.map((protocol) => (
        <TabsContent
          key={protocol.id}
          value={protocol.id}
          className="mt-0 focus-visible:outline-none"
        >
          <ProtocolPanel protocol={protocol} basePath={basePath} />
        </TabsContent>
      ))}
    </Tabs>
  );
}

const NO_ENGINE_CONFIG = {};

interface MesioFormProps {
  basePath?: string;
  /** Edit a template's override of an engine instead of the engine itself. */
  isOverride?: boolean;
  /** The overridden engine's configuration, whose values unset override fields inherit. */
  engineConfig?: Partial<MesioConfig>;
}

export function MesioForm({
  basePath = 'config',
  isOverride = false,
  engineConfig,
}: MesioFormProps) {
  if (!isOverride) return <MesioSettings basePath={basePath} />;
  return (
    <EngineOverrideProvider
      basePath={basePath}
      engineConfig={
        (engineConfig ?? NO_ENGINE_CONFIG) as Record<string, unknown>
      }
      schema={MesioConfigOverrideSchema}
    >
      <MesioSettings basePath={basePath} />
    </EngineOverrideProvider>
  );
}
