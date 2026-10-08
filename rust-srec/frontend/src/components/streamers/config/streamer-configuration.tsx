import { useMemo } from 'react';
import { useWatch } from 'react-hook-form';
import type { UseFormReturn } from 'react-hook-form';
import { Boxes } from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { EngineConfig, StreamerFormValues } from '@/api/schemas';
import type {
  CredentialOwner,
  CredentialPlatform,
} from '@/api/schemas/credential-profiles';
import { SharedConfigEditor } from '../../config/shared-config-editor';
import type { ProxyRouteInherit } from '../../config/shared/proxy-route-picker';
import { PlatformSpecificTab } from '../../config/platforms/tabs/platform-specific-tab';
import { usePlatformDetection } from '@/hooks/use-platform-detection';

interface StreamerConfigurationProps {
  form: UseFormReturn<StreamerFormValues>;
  engines?: EngineConfig[];
  streamerId?: string;
  /** The saved streamer's platform, whose accounts it selects from; keep it memoized. */
  credentialPlatform?: CredentialPlatform;
}

export function StreamerConfiguration({
  form,
  engines,
  streamerId,
  credentialPlatform,
}: StreamerConfigurationProps) {
  // `streamer_specific_config` is a nested object in the form state; every path below hangs off it.
  const basePath = 'streamer_specific_config';

  // The platform-options fields are per-platform, so they follow whatever the URL currently
  // resolves to rather than the streamer's stored platform.
  const url = useWatch({ control: form.control, name: 'url' });
  const { platform } = usePlatformDetection(url);

  // Inheriting takes the template's route on the streamer's platform, or the
  // platform's when there is no template. Until the streamer is saved its
  // platform is not known.
  const templateId = useWatch({ control: form.control, name: 'template_id' });
  const platformId = credentialPlatform?.id;
  const proxyInherit = useMemo((): ProxyRouteInherit => {
    if (!platformId) return { kind: 'unknown' };
    if (templateId && templateId !== 'none')
      return {
        kind: 'scope',
        query: {
          scope_type: 'template',
          scope_id: templateId,
          platform_id: platformId,
        },
      };
    return {
      kind: 'scope',
      query: { scope_type: 'platform', scope_id: platformId },
    };
  }, [platformId, templateId]);

  // Memoized because the shared editor forwards it to a memoized card.
  const credentialScope = useMemo(
    (): CredentialOwner | undefined =>
      streamerId ? { type: 'streamer', streamer_id: streamerId } : undefined,
    [streamerId],
  );

  return (
    <SharedConfigEditor
      form={form}
      engines={engines}
      paths={{
        streamSelection: `${basePath}.stream_selection_config`,
        credentialSelection: `${basePath}.credential_selection`,
        proxyRoute: `${basePath}.proxy_route`,
        retryPolicy: `${basePath}.download_retry_policy`,
        output: basePath, // output_folder etc are in structure
        limits: basePath, // limits are in structure
        danmu: basePath, // record_danmu is in structure
        danmuStatistics: basePath,
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
              inherited
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
      proxyInherit={proxyInherit}
      credentialScope={credentialScope}
      credentialPlatform={credentialPlatform}
    />
  );
}
