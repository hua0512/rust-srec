import { type RshipConfigSchema } from '../processor-schemas';
import { z } from 'zod';
import { get, useFormState } from 'react-hook-form';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';
import { FlagFormField } from '@/components/ui/flag-form-field';
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
  HardDrive,
  FolderOpen,
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
  UploadProxySection,
} from './processor-form-layout';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { msg } from '@lingui/core/macro';
import { FieldHint } from './field-hint';
import { PLACEHOLDER_TOKENS } from '../../constants';

type RshipConfig = z.infer<typeof RshipConfigSchema>;

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
          <Trans>(Update batch)</Trans>
        </span>
      </div>
    ),
  },
];

const GENERAL_FIELDS: ReadonlyArray<keyof RshipConfig> = [
  'operation',
  'destination_root',
  'time_anchor',
  'checksum',
  'strict_source',
  'proxy_route',
];

const ADVANCED_FIELDS: ReadonlyArray<keyof RshipConfig> = [
  'binary_path',
  'config_path',
  'state_dir',
  'max_retries',
  'drive_stop_on_upload_limit',
  'bwlimit',
  'transfers',
  'part_concurrency',
  'adaptive',
  'chunk_size',
  'max_buffer_mem',
  'tpslimit',
  'args',
];

export function RshipConfigForm({
  control,
  pathPrefix,
}: ProcessorConfigFormProps) {
  const { i18n } = useLingui();
  const fieldPath = useProcessorFieldPath<RshipConfig>(pathPrefix);
  const { errors } = useFormState({ control });
  const hasGeneralError = GENERAL_FIELDS.some((key) =>
    Boolean(get(errors, fieldPath(key))),
  );
  const hasAdvancedError = ADVANCED_FIELDS.some((key) =>
    Boolean(get(errors, fieldPath(key))),
  );

  return (
    <Tabs defaultValue="general" className="w-full">
      <TabsList className="grid w-full grid-cols-2 mb-4 bg-muted/20 p-1">
        <TabsTrigger
          value="general"
          aria-invalid={hasGeneralError || undefined}
          className="data-[state=active]:bg-background data-[state=active]:shadow-sm"
        >
          <Trans>General</Trans>
          {hasGeneralError && (
            <span
              className="ml-1.5 size-2 rounded-full bg-destructive"
              aria-hidden="true"
            />
          )}
        </TabsTrigger>
        <TabsTrigger
          value="advanced"
          aria-invalid={hasAdvancedError || undefined}
          className="data-[state=active]:bg-background data-[state=active]:shadow-sm"
        >
          <Trans>Advanced</Trans>
          {hasAdvancedError && (
            <span
              className="ml-1.5 size-2 rounded-full bg-destructive"
              aria-hidden="true"
            />
          )}
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
              <Trans>
                Choose how files are transferred to the remote. Sync requires
                multiple batch inputs in a shared directory and updates those
                files on the remote; other remote files are left untouched.
              </Trans>
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
                    Configure remotes in rship's config.toml before running
                    uploads (for example via rship auth or rship config add).
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
            placeholder={i18n._(msg`e.g. gdrive:archive`)}
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
          <FlagFormField
            control={control}
            fieldName={fieldPath('checksum')}
            title={<Trans>Compare by checksum</Trans>}
            description={
              <Trans>
                Compare existing files by hash instead of size and modification
                time (--checksum).
              </Trans>
            }
          />
          <FlagFormField
            control={control}
            fieldName={fieldPath('strict_source')}
            title={<Trans>Snapshot source files before upload</Trans>}
            description={
              <Trans>
                Freeze source files into temporary local snapshots so concurrent
                writes cannot mutate an upload in flight (--strict-source).
              </Trans>
            }
          />
        </ProcessorSection>

        <UploadProxySection control={control} name={fieldPath('proxy_route')} />
      </TabsContent>

      <TabsContent value="advanced" className="space-y-4">
        <ProcessorSection
          icon={FolderOpen}
          iconClassName="text-primary"
          title={<Trans>Tool Location</Trans>}
        >
          <ProcessorTextField
            control={control}
            name={fieldPath('binary_path')}
            label={<Trans>rship Executable (Optional)</Trans>}
            description={
              <Trans>
                Falls back to the RSHIP_PATH environment variable, then PATH
                lookup.
              </Trans>
            }
            placeholder="rship"
            className="font-mono text-sm"
          />
          <ProcessorTextField
            control={control}
            name={fieldPath('config_path')}
            label={<Trans>Config Path (Optional)</Trans>}
            description={
              <Trans>
                Path to rship's config.toml (--config). Uses RSHIP_CONFIG or
                rship's default location when empty.
              </Trans>
            }
            placeholder="/app/config/rship/config.toml"
            className="font-mono text-sm"
          />
          <ProcessorTextField
            control={control}
            name={fieldPath('state_dir')}
            label={<Trans>State Directory (Optional)</Trans>}
            description={
              <Trans>
                Directory for rship's SQLite WAL resume checkpoints and OAuth
                tokens (--state-dir). Uses RSHIP_STATE_DIR or rship's default
                location when empty.
              </Trans>
            }
            placeholder="/app/config/rship/state"
            className="font-mono text-sm"
          />
        </ProcessorSection>

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
              <Trans>
                Number of attempts per job run before failing the upload.
                Ignored when the workflow step sets Attempts above 1.
              </Trans>
            }
            min={1}
            step={1}
            placeholder="3"
          />
          <FlagFormField
            control={control}
            fieldName={fieldPath('drive_stop_on_upload_limit')}
            title={<Trans>Stop on Google Drive daily upload limit</Trans>}
            description={
              <Trans>
                Abort immediately when Google Drive returns its 750 GiB daily
                upload limit so the upload slot is freed
                (--drive-stop-on-upload-limit).
              </Trans>
            }
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
                description={
                  <Trans>
                    Sustained transfer bandwidth cap (--bwlimit), e.g. 20M,
                    500K, 1G.
                  </Trans>
                }
                placeholder={i18n._(msg`e.g. 20M`)}
                className="font-mono text-sm"
              />
            </div>

            <div className="space-y-3 border-t border-border/30 pt-5">
              <ProcessorSubsectionHeader icon={Layers}>
                <Trans>Concurrency</Trans>
              </ProcessorSubsectionHeader>
              <div className="grid grid-cols-1 sm:grid-cols-2 gap-4">
                <ProcessorNumberField
                  control={control}
                  name={fieldPath('transfers')}
                  label={<Trans>Transfers</Trans>}
                  description={
                    <Trans>Concurrent file transfers (--transfers).</Trans>
                  }
                  min={1}
                  step={1}
                  placeholder={i18n._(msg`rship default: 4`)}
                />
                <ProcessorNumberField
                  control={control}
                  name={fieldPath('part_concurrency')}
                  label={<Trans>Part Concurrency</Trans>}
                  description={
                    <Trans>
                      Parallel multipart streams per file on S3, Azure Blob and
                      local storage (--part-concurrency).
                    </Trans>
                  }
                  min={1}
                  step={1}
                  placeholder={i18n._(msg`e.g. 4`)}
                />
              </div>
              <FlagFormField
                control={control}
                fieldName={fieldPath('adaptive')}
                title={<Trans>Adaptive concurrency</Trans>}
                description={
                  <Trans>
                    Dynamically adjust concurrency across small and large file
                    lanes based on throughput and throttling (--adaptive).
                  </Trans>
                }
              />
            </div>

            <div className="space-y-3 border-t border-border/30 pt-5">
              <ProcessorSubsectionHeader icon={HardDrive}>
                <Trans>Memory & Chunking</Trans>
              </ProcessorSubsectionHeader>
              <div className="grid grid-cols-1 sm:grid-cols-2 gap-4">
                <ProcessorTextField
                  control={control}
                  name={fieldPath('chunk_size')}
                  label={<Trans>Chunk Size</Trans>}
                  description={
                    <Trans>Upload chunk size override (--chunk-size).</Trans>
                  }
                  placeholder={i18n._(msg`e.g. 64M`)}
                  className="font-mono text-sm"
                />
                <ProcessorTextField
                  control={control}
                  name={fieldPath('max_buffer_mem')}
                  label={<Trans>Max Buffer Memory</Trans>}
                  description={
                    <Trans>
                      Hard cap on rship's transfer buffer pool
                      (--max-buffer-mem).
                    </Trans>
                  }
                  placeholder={i18n._(msg`e.g. 64M`)}
                  className="font-mono text-sm"
                />
              </div>
            </div>

            <div className="space-y-3 border-t border-border/30 pt-5">
              <ProcessorSubsectionHeader icon={Timer}>
                <Trans>Rate Limiting</Trans>
              </ProcessorSubsectionHeader>
              <ProcessorNumberField
                control={control}
                name={fieldPath('tpslimit')}
                label={<Trans>TPS Limit</Trans>}
                description={
                  <Trans>
                    Max API requests per second (--tpslimit). Leave empty or set
                    to 0 for unlimited.
                  </Trans>
                }
                min={0}
                step="any"
                placeholder={i18n._(msg`0 = unlimited`)}
              />
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
            placeholder={i18n._(msg`Add rship argument`)}
          />
        </ProcessorSection>
      </TabsContent>
    </Tabs>
  );
}
