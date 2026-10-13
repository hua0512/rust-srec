import React from 'react';
import { Trans } from '@lingui/react/macro';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';
import {
  MesioConfigOverrideSchema,
  type MesioConfig,
} from '@/api/schemas/engine';
import { OverrideScope, useEffectiveValue } from './setting-fields';
import {
  AdvancedSections,
  ChangedBadge,
  FeatureSwitch,
  SettingsSection,
} from './setting-sections';
import {
  MESIO_PROTOCOLS,
  protocolFields,
  type MesioSectionSpec,
  type ProtocolSpec,
} from './mesio-sections';

const ProtocolPanel = React.memo(function ProtocolPanel({
  protocol,
  basePath,
}: {
  protocol: ProtocolSpec;
  basePath: string;
}) {
  const fixing = useEffectiveValue(
    `${basePath}.${protocol.fix.path}`,
    protocol.fix.fallback,
  );
  const disabled = (section: MesioSectionSpec) =>
    Boolean(section.requiresFix) && !fixing;
  const name = protocol.name;
  const notice = (
    <Trans>{name} fixing is off, so these settings have no effect.</Trans>
  );

  return (
    <div className="space-y-6">
      <FeatureSwitch field={protocol.fix} basePath={basePath} />
      <SettingsSection
        section={protocol.essentials}
        basePath={basePath}
        disabled={disabled(protocol.essentials)}
        notice={disabled(protocol.essentials) ? notice : undefined}
      />
      <AdvancedSections
        sections={protocol.advanced}
        basePath={basePath}
        disabled={disabled}
        notice={notice}
      />
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
  return (
    <OverrideScope
      isOverride={isOverride}
      basePath={basePath}
      engineConfig={
        (engineConfig ?? NO_ENGINE_CONFIG) as Record<string, unknown>
      }
      schema={MesioConfigOverrideSchema}
    >
      <MesioSettings basePath={basePath} />
    </OverrideScope>
  );
}
