import { Control, UseFormRegister } from 'react-hook-form';
import { RemuxConfigForm } from './remux-config-form';
import { RcloneConfigForm } from './rclone-config-form';
import { ThumbnailConfigForm } from './thumbnail-config-form';
import { AudioExtractConfigForm } from './audio-extract-config-form';
import { CompressionConfigForm } from './compression-config-form';
import { CopyMoveConfigForm } from './copy-move-config-form';
import { DeleteConfigForm } from './delete-config-form';
import { MetadataConfigForm } from './metadata-config-form';
import { ExecuteConfigForm } from './execute-config-form';

interface ProcessorConfigManagerProps {
  processorType: string;
  control: Control<any>;
  register?: UseFormRegister<any>;
  pathPrefix?: string;
}

export function ProcessorConfigManager({
  processorType,
  control,
  pathPrefix,
}: ProcessorConfigManagerProps) {
  // const { getValues, setValue, watch } = useFormContext();
  // Actually, the individual forms take `control` as prop.
  // We should ensure we pass `pathPrefix`.

  // If control is not passed in props, we might want to use useFormContext's control,
  // but the props define it as optional or required?
  // Looking at props: interface ProcessorConfigManagerProps { ... control: Control<any>; ... }
  // It seems control is passed.

  switch (processorType) {
    case 'execute':
      return <ExecuteConfigForm control={control} pathPrefix={pathPrefix} />;
    case 'remux':
      return <RemuxConfigForm control={control} pathPrefix={pathPrefix} />;
    case 'upload':
    case 'rclone':
      return <RcloneConfigForm control={control} pathPrefix={pathPrefix} />;

    case 'thumbnail':
      return <ThumbnailConfigForm control={control} pathPrefix={pathPrefix} />;
    case 'audio_extract':
      return (
        <AudioExtractConfigForm control={control} pathPrefix={pathPrefix} />
      );
    case 'compression':
      return (
        <CompressionConfigForm control={control} pathPrefix={pathPrefix} />
      );
    case 'copy_move':
      return <CopyMoveConfigForm control={control} pathPrefix={pathPrefix} />;
    case 'delete':
      return <DeleteConfigForm control={control} pathPrefix={pathPrefix} />;
    case 'metadata':
      return <MetadataConfigForm control={control} pathPrefix={pathPrefix} />;
    default:
      return (
        <div className="text-muted-foreground italic">
          No configuration available for this processor.
        </div>
      );
  }
}
