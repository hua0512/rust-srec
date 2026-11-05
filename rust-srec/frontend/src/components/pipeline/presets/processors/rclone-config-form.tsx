import { type RcloneConfigSchema } from '../processor-schemas';
import { z } from 'zod';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';
import {
  Cloud,
  Settings2,
  Terminal,
  ArrowRightLeft,
  Copy,
  Move,
  RefreshCw,
  Gauge,
  Activity,
  Layers,
  Timer,
  Split,
} from 'lucide-react';
import {
  ProcessorConfigFormProps,
  useProcessorFieldPath,
} from './common-props';
import {
  ProcessorListField,
  ProcessorNumberField,
  ProcessorSection,
  ProcessorSelectField,
  ProcessorSubsectionHeader,
  ProcessorTextField,
  TimeAnchorField,
} from './processor-form-layout';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { msg } from '@lingui/core/macro';
import { FieldHint } from './field-hint';
import { PLACEHOLDER_TOKENS } from '../../constants';

type RcloneConfig = z.infer<typeof RcloneConfigSchema>;

const OPERATIONS = [
  {
    value: 'copy',
    label: (
      <div className="flex items-center gap-2">
        <Copy className="h-4 w-4 text-green-400" />
        <span>
          <Trans>Copy</Trans>
        </span>
        <span className="ml-2 text-xs text-muted-foreground/50">
          <Trans>(Preserve source)</Trans>
        </span>
      </div>
    ),
  },
  {
    value: 'move',
    label: (
      <div className="flex items-center gap-2">
        <Move className="h-4 w-4 text-orange-400" />
        <span>
          <Trans>Move</Trans>
        </span>
        <span className="ml-2 text-xs text-muted-foreground/50">
          <Trans>(Delete source)</Trans>
        </span>
      </div>
    ),
  },
  {
    value: 'sync',
    label: (
      <div className="flex items-center gap-2">
        <RefreshCw className="h-4 w-4 text-blue-400" />
        <span>
          <Trans>Sync</Trans>
        </span>
        <span className="ml-2 text-xs text-muted-foreground/50">
          <Trans>(Mirror source)</Trans>
        </span>
      </div>
    ),
  },
];

export function RcloneConfigForm({
  control,
  pathPrefix,
}: ProcessorConfigFormProps) {
  const { i18n } = useLingui();
  const fieldPath = useProcessorFieldPath<RcloneConfig>(pathPrefix);

  return (
    <Tabs defaultValue="general" className="w-full">
      <TabsList className="grid w-full grid-cols-2 mb-4 bg-muted/20 p-1">
        <TabsTrigger
          value="general"
          className="data-[state=active]:bg-background data-[state=active]:shadow-sm"
        >
          <Trans>General</Trans>
        </TabsTrigger>
        <TabsTrigger
          value="advanced"
          className="data-[state=active]:bg-background data-[state=active]:shadow-sm"
        >
          <Trans>Advanced</Trans>
        </TabsTrigger>
      </TabsList>

      <TabsContent value="general" className="space-y-4">
        <ProcessorSection
          icon={ArrowRightLeft}
          iconClassName="text-primary"
          title={<Trans>Operation Mode</Trans>}
        >
          <ProcessorSelectField
            control={control}
            name={fieldPath('operation')}
            label={<Trans>Operation</Trans>}
            placeholder={i18n._(msg`Select operation`)}
            description={
              <Trans>Choose how files are transferred to the remote.</Trans>
            }
            options={OPERATIONS}
          />
        </ProcessorSection>

        <ProcessorSection
          icon={Cloud}
          iconClassName="text-primary"
          title={<Trans>Target Configuration</Trans>}
        >
          <ProcessorTextField
            control={control}
            name={fieldPath('destination_root')}
            label={<Trans>Destination Root</Trans>}
            hint={
              <FieldHint label={i18n._(msg`Destination Root help`)}>
                <p>
                  <Trans>
                    Supports placeholders: {PLACEHOLDER_TOKENS} and time tokens
                    like %Y/%m/%d.
                  </Trans>
                </p>
                <p>
                  <Trans>
                    Keep date and time tokens in your filename template when
                    multiple sessions share one destination folder.
                  </Trans>
                </p>
              </FieldHint>
            }
            description={
              <Trans>
                Base path for remote storage. Supports metadata and time
                placeholders.
              </Trans>
            }
            placeholder={i18n._(msg`e.g. gdrive:/videos`)}
            className="font-mono text-sm"
          />
          <TimeAnchorField
            control={control}
            name={fieldPath('time_anchor')}
            description={
              <Trans>
                Use stream start time to keep a midnight-crossing stream in one
                dated folder and group all segments of a session together.
              </Trans>
            }
          />
          <ProcessorTextField
            control={control}
            name={fieldPath('config_path')}
            label={<Trans>Config Path (Optional)</Trans>}
            placeholder="/path/to/rclone.conf"
          />
        </ProcessorSection>
      </TabsContent>

      <TabsContent value="advanced" className="space-y-4">
        <ProcessorSection
          icon={Settings2}
          iconClassName="text-primary"
          title={<Trans>Retry Policy</Trans>}
        >
          <ProcessorNumberField
            control={control}
            name={fieldPath('max_retries')}
            label={<Trans>Max Retries</Trans>}
            description={
              <Trans>Number of attempts before failing the upload.</Trans>
            }
            min={1}
            step={1}
            placeholder="3"
          />
        </ProcessorSection>

        <ProcessorSection
          icon={Gauge}
          iconClassName="text-primary"
          title={<Trans>Throughput</Trans>}
        >
          <div className="space-y-6">
            <div className="space-y-3">
              <ProcessorSubsectionHeader icon={Activity}>
                <Trans>Bandwidth</Trans>
              </ProcessorSubsectionHeader>
              <ProcessorTextField
                control={control}
                name={fieldPath('bwlimit')}
                label={<Trans>Bandwidth Limit</Trans>}
                hint={
                  <FieldHint label={i18n._(msg`Bandwidth Limit help`)}>
                    <p>
                      <Trans>
                        Units are bytes (default base KiB/s). Use suffixes B, K,
                        M, G, T, P for larger sizes.
                      </Trans>
                    </p>
                    <p>
                      <Trans>Examples:</Trans>
                    </p>
                    <ul className="list-disc space-y-0.5 pl-4">
                      <li>
                        <code>10M</code> —{' '}
                        <Trans>both directions at 10 MiB/s</Trans>
                      </li>
                      <li>
                        <code>10M:100k</code> —{' '}
                        <Trans>upload : download (asymmetric)</Trans>
                      </li>
                      <li>
                        <code>08:00,512k 23:00,off</code> —{' '}
                        <Trans>time-of-day timetable</Trans>
                      </li>
                    </ul>
                    <p className="text-muted-foreground/70">
                      <Trans>Tip: 1 Mbit/s ≈ 0.125M (divide bits by 8).</Trans>
                    </p>
                  </FieldHint>
                }
                description={
                  <Trans>
                    Caps overall transfer bandwidth (rclone --bwlimit).
                  </Trans>
                }
                placeholder={i18n._(msg`e.g. 10M`)}
                className="font-mono text-sm"
              />
              <ProcessorTextField
                control={control}
                name={fieldPath('bwlimit_file')}
                label={<Trans>Per-File Bandwidth Limit</Trans>}
                hint={
                  <FieldHint label={i18n._(msg`Per-File Bandwidth Limit help`)}>
                    <p>
                      <Trans>
                        Limits each individual file's transfer rate rather than
                        the overall total.
                      </Trans>
                    </p>
                    <p>
                      <Trans>
                        Composes with Bandwidth Limit — useful to keep one big
                        file from saturating the whole link while still allowing
                        many small ones to run in parallel.
                      </Trans>
                    </p>
                  </FieldHint>
                }
                description={
                  <Trans>
                    Per-file cap (rclone --bwlimit-file). Same syntax as
                    Bandwidth Limit; composes with it.
                  </Trans>
                }
                placeholder={i18n._(msg`e.g. 1M`)}
                className="font-mono text-sm"
              />
            </div>

            <div className="space-y-3 border-t border-border/30 pt-5">
              <ProcessorSubsectionHeader icon={Layers}>
                <Trans>Concurrency</Trans>
              </ProcessorSubsectionHeader>
              <div className="grid grid-cols-2 gap-4">
                <ProcessorNumberField
                  control={control}
                  name={fieldPath('transfers')}
                  label={<Trans>Transfers</Trans>}
                  hint={
                    <FieldHint label={i18n._(msg`Transfers help`)}>
                      <p>
                        <Trans>
                          How many files rclone uploads in parallel.
                        </Trans>
                      </p>
                      <p>
                        <Trans>
                          Higher = faster on fast connections, but more
                          aggressive on the remote and your CPU. Increase only
                          if your network and provider can handle it.
                        </Trans>
                      </p>
                    </FieldHint>
                  }
                  description={
                    <Trans>Concurrent file transfers (--transfers).</Trans>
                  }
                  min={1}
                  step={1}
                  placeholder={i18n._(msg`rclone default: 4`)}
                />
                <ProcessorNumberField
                  control={control}
                  name={fieldPath('checkers')}
                  label={<Trans>Checkers</Trans>}
                  hint={
                    <FieldHint label={i18n._(msg`Checkers help`)}>
                      <p>
                        <Trans>
                          Parallel integrity checks (e.g. comparing source /
                          destination hashes).
                        </Trans>
                      </p>
                      <p>
                        <Trans>
                          Cheap operations, so the default of 8 is usually fine.
                          Increase if you have many small files against a slow
                          remote.
                        </Trans>
                      </p>
                    </FieldHint>
                  }
                  description={<Trans>Concurrent checkers (--checkers).</Trans>}
                  min={1}
                  step={1}
                  placeholder={i18n._(msg`rclone default: 8`)}
                />
              </div>
            </div>

            <div className="space-y-3 border-t border-border/30 pt-5">
              <ProcessorSubsectionHeader icon={Timer}>
                <Trans>Rate Limiting</Trans>
              </ProcessorSubsectionHeader>
              <div className="grid grid-cols-2 gap-4">
                <ProcessorNumberField
                  control={control}
                  name={fieldPath('tpslimit')}
                  label={<Trans>TPS Limit</Trans>}
                  hint={
                    <FieldHint label={i18n._(msg`TPS Limit help`)}>
                      <p>
                        <Trans>
                          Rate-limits API calls to the remote. Useful when a
                          provider returns 429 "too many requests" under heavy
                          load.
                        </Trans>
                      </p>
                      <p>
                        <Trans>
                          <code>0</code> means unlimited (rclone default).
                          Example: set to <code>10</code> if the provider caps
                          you at 10 requests/second.
                        </Trans>
                      </p>
                    </FieldHint>
                  }
                  description={
                    <Trans>
                      Max transactions/second to the remote API (--tpslimit).
                    </Trans>
                  }
                  min={0}
                  step={0.1}
                  placeholder={i18n._(msg`0 = unlimited`)}
                />
                <ProcessorNumberField
                  control={control}
                  name={fieldPath('tpslimit_burst')}
                  label={<Trans>TPS Burst</Trans>}
                  hint={
                    <FieldHint label={i18n._(msg`TPS Burst help`)}>
                      <p>
                        <Trans>
                          How many requests can briefly burst above TPS Limit
                          before throttling kicks in.
                        </Trans>
                      </p>
                      <p>
                        <Trans>
                          Default is 1 (no burst). Increase if the provider
                          tolerates short bursts of activity.
                        </Trans>
                      </p>
                    </FieldHint>
                  }
                  description={
                    <Trans>
                      Burst capacity for TPS Limit (--tpslimit-burst).
                    </Trans>
                  }
                  min={0}
                  step={1}
                  placeholder={i18n._(msg`rclone default: 1`)}
                />
              </div>
            </div>

            <div className="space-y-3 border-t border-border/30 pt-5">
              <ProcessorSubsectionHeader icon={Split}>
                <Trans>Multi-Thread Copy</Trans>
              </ProcessorSubsectionHeader>
              <div className="grid grid-cols-2 gap-4">
                <ProcessorNumberField
                  control={control}
                  name={fieldPath('multi_thread_streams')}
                  label={<Trans>Streams</Trans>}
                  hint={
                    <FieldHint label={i18n._(msg`Multi-Thread Streams help`)}>
                      <p>
                        <Trans>
                          Splits a single large file into N parallel chunks for
                          faster upload.
                        </Trans>
                      </p>
                      <p>
                        <Trans>
                          Only kicks in for files above the Cutoff size. Default
                          is 4. Set to 0 to disable.
                        </Trans>
                      </p>
                    </FieldHint>
                  }
                  description={
                    <Trans>Streams per file (--multi-thread-streams).</Trans>
                  }
                  min={0}
                  step={1}
                  placeholder={i18n._(msg`rclone default: 4`)}
                />
                <ProcessorTextField
                  control={control}
                  name={fieldPath('multi_thread_cutoff')}
                  label={<Trans>Cutoff</Trans>}
                  hint={
                    <FieldHint label={i18n._(msg`Multi-Thread Cutoff help`)}>
                      <p>
                        <Trans>
                          Files smaller than this stay single-threaded.
                          Examples: <code>250M</code>, <code>1G</code>.
                        </Trans>
                      </p>
                      <p>
                        <Trans>
                          Multi-thread upload only helps on large files; on
                          small ones the splitting overhead outweighs the
                          benefit.
                        </Trans>
                      </p>
                    </FieldHint>
                  }
                  description={
                    <Trans>File size threshold (--multi-thread-cutoff).</Trans>
                  }
                  placeholder={i18n._(msg`e.g. 250M`)}
                  className="font-mono text-sm"
                />
              </div>
            </div>
          </div>
        </ProcessorSection>

        <ProcessorSection
          icon={Terminal}
          iconClassName="text-primary"
          title={<Trans>Extra Arguments</Trans>}
        >
          <ProcessorListField
            control={control}
            name={fieldPath('args')}
            description={<Trans>Double click to edit items.</Trans>}
            placeholder={i18n._(msg`Add rclone argument`)}
          />
        </ProcessorSection>
      </TabsContent>
    </Tabs>
  );
}
