import { UseFormReturn } from 'react-hook-form';
import { EngineConfig } from '@/api/schemas';
import { SharedConfigEditor } from '../../config/shared-config-editor';

interface StreamerConfigurationProps {
  form: UseFormReturn<any>;
  engines?: EngineConfig[];
  streamerId?: string;
  credentialPlatformNameHint?: string;
}

export function StreamerConfiguration({
  form,
  engines,
  streamerId,
  credentialPlatformNameHint,
}: StreamerConfigurationProps) {
  // We can directly watch form values if needed, but shared components bind directly to form context.
  // The 'streamer_specific_config' is now a nested object in the form state.
  const basePath = 'streamer_specific_config';

  return (
    <SharedConfigEditor
      form={form}
      engines={engines}
      paths={{
        streamSelection: `${basePath}.stream_selection_config`,
        cookies: `${basePath}.cookies`,
        proxy: `${basePath}.proxy_config`,
        retryPolicy: `${basePath}.download_retry_policy`,
        output: basePath, // output_folder etc are in structure
        limits: basePath, // limits are in structure
        danmu: basePath, // record_danmu is in structure
        danmuSampling: `${basePath}.danmu_sampling_config`,
        hooks: `${basePath}.event_hooks`,
        pipeline: `${basePath}.pipeline`,
        sessionCompletePipeline: `${basePath}.session_complete_pipeline`,
        pairedSegmentPipeline: `${basePath}.paired_segment_pipeline`,
      }}
      configMode="object"
      proxyMode="object"
      streamerId={streamerId}
      credentialPlatformNameHint={credentialPlatformNameHint}
    />
  );
}
