import { z } from 'zod';
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
import { RemuxConfigForm } from './remux-config-form';
import { RcloneConfigForm } from './rclone-config-form';
import { ThumbnailConfigForm } from './thumbnail-config-form';
import { AudioExtractConfigForm } from './audio-extract-config-form';
import { CompressionConfigForm } from './compression-config-form';
import { CopyMoveConfigForm } from './copy-move-config-form';
import { DeleteConfigForm } from './delete-config-form';
import { MetadataConfigForm } from './metadata-config-form';
import { ExecuteConfigForm } from './execute-config-form';

import { ComponentType } from 'react';
import { ProcessorConfigFormProps } from './common-props';

export interface ProcessorDefinition {
  schema: z.ZodType<any>;
  component: ComponentType<ProcessorConfigFormProps<any>>;
  label: string;
}

export const PROCESSOR_REGISTRY: Record<string, ProcessorDefinition> = {
  remux: {
    schema: RemuxConfigSchema,
    component: RemuxConfigForm,
    label: 'Remux / Transcode',
  },
  rclone: {
    schema: RcloneConfigSchema,
    component: RcloneConfigForm,
    label: 'Rclone Transfer',
  },
  thumbnail: {
    schema: ThumbnailConfigSchema,
    component: ThumbnailConfigForm,
    label: 'Thumbnail Generator',
  },
  audio_extract: {
    schema: AudioExtractConfigSchema,
    component: AudioExtractConfigForm,
    label: 'Audio Extraction',
  },
  compression: {
    schema: CompressionConfigSchema,
    component: CompressionConfigForm,
    label: 'Compression / Archive',
  },
  copy: {
    schema: CopyMoveConfigSchema,
    component: CopyMoveConfigForm,
    label: 'Copy / Move',
  },
  move: {
    schema: CopyMoveConfigSchema,
    component: CopyMoveConfigForm,
    label: 'Copy / Move',
  },
  delete: {
    schema: DeleteConfigSchema,
    component: DeleteConfigForm,
    label: 'Delete File',
  },
  metadata: {
    schema: MetadataConfigSchema,
    component: MetadataConfigForm,
    label: 'Metadata Editor',
  },
  execute: {
    schema: ExecuteConfigSchema,
    component: ExecuteConfigForm,
    label: 'Execute Command',
  },
};

export const getProcessorDefinition = (
  processorName: string,
): ProcessorDefinition | undefined => {
  return PROCESSOR_REGISTRY[processorName];
};
