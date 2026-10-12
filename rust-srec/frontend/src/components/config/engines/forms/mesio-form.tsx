import React from 'react';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Badge } from '@/components/ui/badge';
import {
  Database,
  Wrench,
  Film,
  Settings2,
  RefreshCw,
  Layers,
  Combine,
} from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { MesioHlsForm } from './mesio-hls-form';
import { MesioDashForm } from './mesio-dash-form';
import {
  EngineOverrideProvider,
  SettingInput,
  SettingSelect,
  SettingSwitch,
  useEffectiveValue,
  type SettingOption,
} from './setting-fields';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';
import {
  MesioConfigOverrideSchema,
  resolveDashSettings,
  type MesioConfig,
} from '@/api/schemas/engine';

interface SubFormProps {
  basePath: string;
}

const SEQUENCE_HEADER_MODE_OPTIONS: readonly SettingOption[] = [
  {
    value: 'crc32',
    label: 'crc32',
    content: (
      <div className="flex flex-col gap-0.5">
        <span className="font-medium text-xs">crc32 (Default)</span>
        <span className="text-[10px] text-muted-foreground leading-relaxed max-w-[300px]">
          <Trans>
            Split on any raw header change. Safe but may cause extra files.
          </Trans>
        </span>
      </div>
    ),
  },
  {
    value: 'semantic_signature',
    label: 'semantic_signature',
    content: (
      <div className="flex flex-col gap-0.5">
        <div className="flex items-center gap-2">
          <span className="font-medium text-xs">semantic_signature</span>
          <Badge
            variant="secondary"
            className="text-[8px] h-3.5 px-1 bg-orange-500/10 text-orange-600 border-none font-bold"
          >
            <Trans>NEW</Trans>
          </Badge>
        </div>
        <span className="text-[10px] text-muted-foreground leading-relaxed max-w-[300px]">
          <Trans>
            Split only on codec configuration changes. Reduces false splits.
          </Trans>
        </span>
      </div>
    ),
  },
];

const FLV_SWITCH_ROW =
  'rounded-xl border border-border/40 bg-muted/5 p-4 py-3 shadow-none transition-all hover:bg-muted/10';

const MesioFlvForm = React.memo(({ basePath }: SubFormProps) => {
  const flvPath = `${basePath}.flv_fix`;
  const duplicateTagFiltering = useEffectiveValue(
    `${flvPath}.duplicate_tag_filtering`,
    false,
  );
  const filterPath = `${flvPath}.duplicate_tag_filter_config`;

  return (
    <div className="space-y-4">
      <Card className="border-border/50 shadow-sm">
        <CardContent className="p-4 space-y-6">
          <SettingSelect
            name={`${flvPath}.sequence_header_change_mode`}
            label={<Trans>Stream Splitting Strategy</Trans>}
            icon={RefreshCw}
            options={SEQUENCE_HEADER_MODE_OPTIONS}
            fallback="crc32"
          />

          <div className="space-y-3">
            <SettingSwitch
              name={`${flvPath}.drop_duplicate_sequence_headers`}
              label={<Trans>Optimize Stream Headers</Trans>}
              description={
                <Trans>
                  Suppress redundant headers to reduce player micro-stutter
                </Trans>
              }
              fallback={false}
              size="default"
              className={FLV_SWITCH_ROW}
              switchClassName="scale-90"
            />

            <SettingSwitch
              name={`${flvPath}.duplicate_tag_filtering`}
              label={
                <>
                  <Trans>Loop Protection</Trans>
                  <Badge
                    variant="secondary"
                    className="ml-2 text-[8px] h-3.5 px-1 bg-blue-500/10 text-blue-600 border-none font-bold"
                  >
                    <Trans>BETA</Trans>
                  </Badge>
                </>
              }
              description={
                <Trans>
                  Filter repeated tags and detect stream replay loops
                </Trans>
              }
              fallback={false}
              icon={Layers}
              size="default"
              className={FLV_SWITCH_ROW}
              switchClassName="scale-90"
            />

            {duplicateTagFiltering && (
              <div className="grid gap-3 pt-1">
                <div className="bg-blue-500/5 border border-blue-500/10 rounded-xl p-4 grid gap-4 sm:grid-cols-2">
                  <SettingInput
                    name={`${filterPath}.window_capacity_tags`}
                    label={<Trans>Filter Window Size</Trans>}
                    fallback={8192}
                    size="sm"
                  />
                  <SettingInput
                    name={`${filterPath}.replay_backjump_threshold_ms`}
                    label={<Trans>Backjump Threshold</Trans>}
                    fallback={2000}
                    size="sm"
                  />
                  <div className="sm:col-span-2 pt-1 border-t border-blue-500/10">
                    <SettingSwitch
                      name={`${filterPath}.enable_replay_offset_matching`}
                      label={<Trans>Offset Consistency Check</Trans>}
                      fallback={false}
                    />
                  </div>
                </div>
              </div>
            )}
          </div>
        </CardContent>
      </Card>
    </div>
  );
});
MesioFlvForm.displayName = 'MesioFlvForm';

/** Classes of the three stream-fixing toggles, tinted per protocol. */
function fixToggleClass(accent: string) {
  return `rounded-xl border border-border/40 bg-gradient-to-br from-background/50 p-4 shadow-sm transition-all ${accent}`;
}

function MesioSettings({
  basePath,
  inheritedDash,
}: {
  basePath: string;
  inheritedDash?: ReturnType<typeof resolveDashSettings>;
}) {
  // FLV tuning applies only while FLV fixing is on. The HLS and DASH settings also cover
  // downloading, so they apply whether or not fixing is on.
  const fixFlv = useEffectiveValue(`${basePath}.fix_flv`, true);

  return (
    <Tabs defaultValue="general" className="w-full space-y-4">
      <TabsList className="w-full bg-background/40 border border-border/40 p-1 py-1.5 h-auto overflow-x-auto no-scrollbar justify-start">
        <TabsTrigger value="general" className="flex-1 min-w-[100px] gap-2">
          <Settings2 className="w-4 h-4 text-primary" />
          <Trans>General</Trans>
        </TabsTrigger>
        <TabsTrigger
          value="flv"
          className="flex-1 min-w-[100px] gap-2 disabled:opacity-50"
          disabled={!fixFlv}
        >
          <Film className="w-4 h-4 text-orange-500" />
          <Trans>FLV Tuning</Trans>
          {!fixFlv && (
            <Badge variant="outline" className="text-[8px] h-3 px-1 ml-1">
              Off
            </Badge>
          )}
        </TabsTrigger>
        <TabsTrigger value="hls" className="flex-1 min-w-[100px] gap-2">
          <Wrench className="w-4 h-4 text-blue-500" />
          <Trans>HLS Tuning</Trans>
        </TabsTrigger>
        <TabsTrigger value="dash" className="flex-1 min-w-[100px] gap-2">
          <Combine className="w-4 h-4 text-emerald-500" />
          <Trans>DASH Tuning</Trans>
        </TabsTrigger>
      </TabsList>

      <TabsContent
        value="general"
        className="space-y-6 mt-0 focus-visible:outline-none"
      >
        <Card className="border-border/50 shadow-sm">
          <CardHeader className="pb-3 pt-4 px-4">
            <CardTitle className="text-sm font-medium flex items-center gap-2">
              <Database className="w-4 h-4 text-primary" />
              <Trans>Global Configuration</Trans>
            </CardTitle>
          </CardHeader>
          <CardContent className="px-4 pb-4">
            <SettingInput
              name={`${basePath}.buffer_size`}
              label={<Trans>Global Buffer Size (bytes)</Trans>}
              description={<Trans>Recommended: 8388608 (8 MiB)</Trans>}
              fallback={8388608}
            />
          </CardContent>
        </Card>

        {/* Three across when there is room, otherwise stacked: never two and an orphan. */}
        <div className="@container">
          <div className="grid grid-cols-1 gap-4 @3xl:grid-cols-3">
            <SettingSwitch
              name={`${basePath}.fix_flv`}
              label={<Trans>Fix FLV Streams</Trans>}
              description={
                <Trans>Enable advanced FLV timestamp repairing</Trans>
              }
              fallback={true}
              icon={Film}
              size="default"
              className={fixToggleClass(
                'to-orange-500/5 hover:border-orange-500/20',
              )}
              switchClassName=""
            />
            <SettingSwitch
              name={`${basePath}.fix_hls`}
              label={<Trans>Fix HLS Discontinuities</Trans>}
              description={
                <Trans>Enable advanced HLS segment reconstruction</Trans>
              }
              fallback={true}
              icon={Wrench}
              size="default"
              className={fixToggleClass(
                'to-blue-500/5 hover:border-blue-500/20',
              )}
              switchClassName=""
            />
            <SettingSwitch
              name={`${basePath}.fix_dash`}
              label={<Trans>Fix DASH Streams</Trans>}
              description={
                <Trans>
                  Check merged DASH output and start a new file on stream
                  changes
                </Trans>
              }
              fallback={true}
              icon={Combine}
              size="default"
              className={fixToggleClass(
                'to-emerald-500/5 hover:border-emerald-500/20',
              )}
              switchClassName=""
            />
          </div>
        </div>
      </TabsContent>

      <TabsContent value="flv" className="mt-0 focus-visible:outline-none">
        <MesioFlvForm basePath={basePath} />
      </TabsContent>

      <TabsContent value="hls" className="mt-0 focus-visible:outline-none">
        <MesioHlsForm basePath={basePath} />
      </TabsContent>

      <TabsContent value="dash" className="mt-0 focus-visible:outline-none">
        <MesioDashForm basePath={basePath} inherited={inheritedDash} />
      </TabsContent>
    </Tabs>
  );
}

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
  const inheritedDash = React.useMemo(
    () => (isOverride ? resolveDashSettings(engineConfig?.dash) : undefined),
    [isOverride, engineConfig],
  );

  if (!isOverride) return <MesioSettings basePath={basePath} />;
  return (
    <EngineOverrideProvider
      basePath={basePath}
      engineConfig={(engineConfig ?? {}) as Record<string, unknown>}
      schema={MesioConfigOverrideSchema}
    >
      <MesioSettings basePath={basePath} inheritedDash={inheritedDash} />
    </EngineOverrideProvider>
  );
}
