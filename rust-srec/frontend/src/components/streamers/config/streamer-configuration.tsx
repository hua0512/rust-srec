import { UseFormReturn, useWatch } from 'react-hook-form';
import { Boxes } from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { EngineConfig } from '@/api/schemas';
import { SharedConfigEditor } from '../../config/shared-config-editor';
import { PlatformSpecificTab } from '../../config/platforms/tabs/platform-specific-tab';
import { usePlatformDetection } from '@/hooks/use-platform-detection';

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
  // `streamer_specific_config` is a nested object in the form state; every path below hangs off it.
  const basePath = 'streamer_specific_config';

  // The platform-options fields are per-platform, so they follow whatever the URL currently
  // resolves to rather than the streamer's stored platform.
  const url = useWatch({ control: form.control, name: 'url' });
  const { platform } = usePlatformDetection(url);

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
        pipeline: `${basePath}.pipeline`,
        sessionCompletePipeline: `${basePath}.session_complete_pipeline`,
        pairedSegmentPipeline: `${basePath}.paired_segment_pipeline`,
        offlineCheck: basePath,
      }}
      extraTabs={[
        {
          value: 'platform',
          label: <Trans>Platform options</Trans>,
          icon: Boxes,
          content: (
            <PlatformSpecificTab
              form={form}
              basePath={basePath}
              platformName={platform ?? undefined}
              // The streamer resolver reads these from `platform_extras`, not the
              // `platform_specific_config` key the platform and template rows use.
              field="platform_extras"
            />
          ),
        },
      ]}
      configMode="object"
      proxyMode="object"
      streamerId={streamerId}
      credentialPlatformNameHint={credentialPlatformNameHint}
    />
  );
}
