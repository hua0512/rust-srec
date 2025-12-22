import { z } from 'zod';
import { lazy, ComponentType } from 'react';
import {
  RemuxConfigSchema,
  RcloneConfigSchema,
  ThumbnailConfigSchema,
  AudioExtractConfigSchema,
  CompressionConfigSchema,
  CopyMoveConfigSchema,
  DeleteConfigSchema,
  MetadataConfigSchema,
  ExecuteConfigSchema,
} from '../processor-schemas';

// Lazy load large form components for code splitting
const RemuxConfigForm = lazy(() =>
  import('./remux-config-form').then((m) => ({ default: m.RemuxConfigForm })),
);
const RcloneConfigForm = lazy(() =>
  import('./rclone-config-form').then((m) => ({ default: m.RcloneConfigForm })),
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
import { msg } from '@lingui/core/macro';
import { type MessageDescriptor } from '@lingui/core';

export interface ProcessorDefinition {
  schema: z.ZodType<any>;
  component: ComponentType<ProcessorConfigFormProps<any>>;
  label: MessageDescriptor;
}

export const PROCESSOR_REGISTRY: Record<string, ProcessorDefinition> = {
  remux: {
    schema: RemuxConfigSchema,
    component: RemuxConfigForm,
    label: msg`Remux / Transcode`,
  },
  rclone: {
    schema: RcloneConfigSchema,
    component: RcloneConfigForm,
    label: msg`Rclone Transfer`,
  },
  thumbnail: {
    schema: ThumbnailConfigSchema,
    component: ThumbnailConfigForm,
    label: msg`Thumbnail Generator`,
  },
  audio_extract: {
    schema: AudioExtractConfigSchema,
    component: AudioExtractConfigForm,
    label: msg`Audio Extraction`,
  },
  compression: {
    schema: CompressionConfigSchema,
    component: CompressionConfigForm,
    label: msg`Compression / Archive`,
  },
  copy: {
    schema: CopyMoveConfigSchema,
    component: CopyMoveConfigForm,
    label: msg`Copy / Move`,
  },
  move: {
    schema: CopyMoveConfigSchema,
    component: CopyMoveConfigForm,
    label: msg`Copy / Move`,
  },
  delete: {
    schema: DeleteConfigSchema,
    component: DeleteConfigForm,
    label: msg`Delete File`,
  },
  metadata: {
    schema: MetadataConfigSchema,
    component: MetadataConfigForm,
    label: msg`Metadata Editor`,
  },
  execute: {
    schema: ExecuteConfigSchema,
    component: ExecuteConfigForm,
    label: msg`Execute Command`,
  },
};

export const getProcessorDefinition = (
  processorName: string,
): ProcessorDefinition | undefined => {
  return PROCESSOR_REGISTRY[processorName];
};
