import { z } from 'zod';
import { lazy, ComponentType } from 'react';
import {
  RemuxConfigSchema,
  RcloneConfigSchema,
  BaiduPcsConfigSchema,
  ThumbnailConfigSchema,
  AudioExtractConfigSchema,
  CompressionConfigSchema,
  CopyMoveConfigSchema,
  DeleteConfigSchema,
  MetadataConfigSchema,
  ExecuteConfigSchema,
  DanmakuFactoryConfigSchema,
  AssBurninConfigSchema,
} from '../processor-schemas';

// Lazy load large form components for code splitting
const RemuxConfigForm = lazy(() =>
  import('./remux-config-form').then((m) => ({ default: m.RemuxConfigForm })),
);
const RcloneConfigForm = lazy(() =>
  import('./rclone-config-form').then((m) => ({ default: m.RcloneConfigForm })),
);
const BaiduPcsConfigForm = lazy(() =>
  import('./baidupcs-config-form').then((m) => ({
    default: m.BaiduPcsConfigForm,
  })),
);
const DanmakuFactoryConfigForm = lazy(() =>
  import('./danmaku-factory-config-form').then((m) => ({
    default: m.DanmakuFactoryConfigForm,
  })),
);
const AssBurninConfigForm = lazy(() =>
  import('./ass-burnin-config-form').then((m) => ({
    default: m.AssBurninConfigForm,
  })),
);
// Smaller forms can be imported directly
import { ThumbnailConfigForm } from './thumbnail-config-form';
import { AudioExtractConfigForm } from './audio-extract-config-form';
import { CompressionConfigForm } from './compression-config-form';
import { CopyMoveConfigForm } from './copy-move-config-form';
import { DeleteConfigForm } from './delete-config-form';
import { MetadataConfigForm } from './metadata-config-form';
import { ExecuteConfigForm } from './execute-config-form';

import { ProcessorConfigFormProps } from './common-props';
import { PROCESSOR_META, type ProcessorType } from '../../constants';
import { type MessageDescriptor } from '@lingui/core';

interface ProcessorDefinition {
  schema: z.ZodType<any>;
  component: ComponentType<ProcessorConfigFormProps>;
  label: MessageDescriptor;
}

const PROCESSOR_REGISTRY: Record<ProcessorType, ProcessorDefinition> = {
  remux: {
    ...PROCESSOR_META.remux,
    schema: RemuxConfigSchema,
    component: RemuxConfigForm,
  },
  thumbnail: {
    ...PROCESSOR_META.thumbnail,
    schema: ThumbnailConfigSchema,
    component: ThumbnailConfigForm,
  },
  audio_extract: {
    ...PROCESSOR_META.audio_extract,
    schema: AudioExtractConfigSchema,
    component: AudioExtractConfigForm,
  },
  compression: {
    ...PROCESSOR_META.compression,
    schema: CompressionConfigSchema,
    component: CompressionConfigForm,
  },
  copy_move: {
    ...PROCESSOR_META.copy_move,
    schema: CopyMoveConfigSchema,
    component: CopyMoveConfigForm,
  },
  delete: {
    ...PROCESSOR_META.delete,
    schema: DeleteConfigSchema,
    component: DeleteConfigForm,
  },
  metadata: {
    ...PROCESSOR_META.metadata,
    schema: MetadataConfigSchema,
    component: MetadataConfigForm,
  },
  rclone: {
    ...PROCESSOR_META.rclone,
    schema: RcloneConfigSchema,
    component: RcloneConfigForm,
  },
  baidupcs: {
    ...PROCESSOR_META.baidupcs,
    schema: BaiduPcsConfigSchema,
    component: BaiduPcsConfigForm,
  },
  execute: {
    ...PROCESSOR_META.execute,
    schema: ExecuteConfigSchema,
    component: ExecuteConfigForm,
  },
  danmaku_factory: {
    ...PROCESSOR_META.danmaku_factory,
    schema: DanmakuFactoryConfigSchema,
    component: DanmakuFactoryConfigForm,
  },
  ass_burnin: {
    ...PROCESSOR_META.ass_burnin,
    schema: AssBurninConfigSchema,
    component: AssBurninConfigForm,
  },
};

/**
 * Alternate names the backend also accepts for a processor (each processor's
 * `job_types()` under `rust-srec/src/pipeline/processors`). Steps created
 * through the API or an import may use them.
 */
const PROCESSOR_ALIASES: Record<string, ProcessorType> = {
  transcode: 'remux',
  convert: 'remux',
  upload: 'rclone',
  extract_audio: 'audio_extract',
  compress: 'compression',
  archive: 'compression',
  cleanup: 'delete',
  embed_metadata: 'metadata',
  command: 'execute',
  danmu_to_ass: 'danmaku_factory',
  danmu: 'danmaku_factory',
  burn_ass: 'ass_burnin',
  burn_subtitles: 'ass_burnin',
};

export const getProcessorDefinition = (
  processorName: string,
): ProcessorDefinition | undefined => {
  const registry: Partial<Record<string, ProcessorDefinition>> =
    PROCESSOR_REGISTRY;
  return registry[processorName] ?? registry[PROCESSOR_ALIASES[processorName]];
};

/**
 * The config a processor runs with when nothing is set: every schema default
 * applied to an empty object. `undefined` without a definition or when the
 * schema cannot be satisfied without user input.
 */
export function getProcessorDefaultConfig(
  definition: ProcessorDefinition | null | undefined,
): Record<string, unknown> | undefined {
  const result = definition?.schema.safeParse({});
  return result?.success ? result.data : undefined;
}
