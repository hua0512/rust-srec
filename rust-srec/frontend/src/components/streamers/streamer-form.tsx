import { useForm, SubmitHandler } from 'react-hook-form';
import { zodResolver } from '@hookform/resolvers/zod';
import { PlatformConfigSchema } from '@/api/schemas';
import { useQuery } from '@tanstack/react-query';
import {
  listPlatformConfigs,
  listTemplates,
  extractMetadata,
  listEngines,
  parseUrl,
} from '@/server/functions';
import { Button } from '@/components/ui/button';
import { toast } from 'sonner';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { Form } from '@/components/ui/form';
import { z } from 'zod';
import { Trans } from '@lingui/react/macro';
import { Loader2, ArrowRight, ArrowLeft, Undo2 } from 'lucide-react';
import { useState, useCallback } from 'react';
import { StreamerMetaForm } from './edit/streamer-meta-form';
import { StreamerConfigForm } from './edit/streamer-config-form';
import { StreamerSaveFab } from './edit/streamer-save-fab';
import { useNavigate } from '@tanstack/react-router';

import { StreamerFormSchema, StreamerFormValues } from '@/api/schemas';

type PlatformConfig = z.infer<typeof PlatformConfigSchema>;

interface StreamerFormProps {
  defaultValues?: Partial<StreamerFormValues>;
  onSubmit: SubmitHandler<StreamerFormValues>;
  isSubmitting: boolean;
  title: React.ReactNode;
  description: React.ReactNode;
  submitLabel?: React.ReactNode;
}

export function StreamerForm({
  defaultValues,
  onSubmit,
  isSubmitting,
  title,
}: StreamerFormProps) {
  const { i18n } = useLingui();
  const navigate = useNavigate();
  const [stage, setStage] = useState<1 | 2>(1);
  const [detectingPlatform, setDetectingPlatform] = useState(false);
  const [isAutofilling, setIsAutofilling] = useState(false);
  const [detectedPlatform, setDetectedPlatform] = useState<string | null>(null);
  const [validPlatformConfigs, setValidPlatformConfigs] = useState<
    PlatformConfig[]
  >([]);

  // Fetch dependencies
  const { data: allPlatforms, isLoading: platformsLoading } = useQuery({
    queryKey: ['platforms'],
    queryFn: () => listPlatformConfigs(),
  });

  const { data: templates, isLoading: templatesLoading } = useQuery({
    queryKey: ['templates'],
    queryFn: () => listTemplates(),
  });

  const { data: engines, isLoading: enginesLoading } = useQuery({
    queryKey: ['engines'],
    queryFn: () => listEngines(),
  });

  // Parse the initial string config into object if it exists
  const initialSpecificConfig = defaultValues?.streamer_specific_config
    ? typeof defaultValues.streamer_specific_config === 'string'
      ? JSON.parse(defaultValues.streamer_specific_config)
      : defaultValues.streamer_specific_config
    : {};

  const defaults: StreamerFormValues = {
    name: defaultValues?.name ?? '',
    url: defaultValues?.url ?? '',
    priority: defaultValues?.priority ?? 'NORMAL',
    enabled: defaultValues?.enabled ?? true,
    platform_config_id: defaultValues?.platform_config_id ?? '',
    template_id: defaultValues?.template_id,
    streamer_specific_config: initialSpecificConfig,
  };

  const form = useForm<StreamerFormValues>({
    resolver: zodResolver(StreamerFormSchema),
    defaultValues: defaults,
    mode: 'onChange', // Validate on change so we can disable Next button if needed
  });

  // Helper to trim URL and validate - returns trimmed URL if valid, null otherwise
  const trimAndValidateUrl = async (): Promise<string | null> => {
    const url = form.getValues('url')?.trim();
    if (!url) return null;
    form.setValue('url', url);

    const urlValid = await form.trigger('url');
    return urlValid ? url : null;
  };

  const handleAutofillName = useCallback(async () => {
    const url = await trimAndValidateUrl();
    if (!url) return;

    setIsAutofilling(true);
    try {
      const response = await parseUrl({ data: { url } });
      if (response.success && response.media_info?.artist) {
        form.setValue('name', response.media_info.artist, {
          shouldDirty: true,
          shouldValidate: true,
        });
        toast.success(i18n._(msg`Name autofilled successfully`));
      } else if (response.error) {
        toast.error(response.error);
      } else {
        toast.error(i18n._(msg`Failed to extract name from URL`));
      }
    } catch (error: any) {
      console.error('Failed to autofill name:', error);
      toast.error(error.message || i18n._(msg`Failed to autofill name`));
    } finally {
      setIsAutofilling(false);
    }
  }, [form]);

  const handleNext = async () => {
    const url = await trimAndValidateUrl();
    if (!url) return;

    // Also validate name for Stage 1
    const nameValid = await form.trigger('name');
    if (!nameValid) return;

    setDetectingPlatform(true);
    try {
      const metadata = await extractMetadata({ data: url });
      setDetectedPlatform(metadata.platform ?? null);
      setValidPlatformConfigs(metadata.valid_platform_configs);

      // Check if we have any platforms to show in Stage 2
      const configs =
        metadata.valid_platform_configs.length > 0
          ? metadata.valid_platform_configs
          : allPlatforms || [];

      if (configs.length === 0) {
        toast.error(
          i18n._(
            msg`No platform configurations found. Please create one first.`,
          ),
        );
        return;
      }

      // If only one valid config and user hasn't selected one, select it
      if (
        metadata.valid_platform_configs.length === 1 &&
        !form.getValues('platform_config_id')
      ) {
        form.setValue(
          'platform_config_id',
          metadata.valid_platform_configs[0].id,
        );
      }

      setStage(2);
    } catch (error) {
      console.error('Failed to extract metadata:', error);
      // Even if extraction fails, let user proceed but show all platforms?
      // Or maybe just show an error toast.
      // For now, let's proceed with all platforms if extraction fails but show warning.
      const configs = allPlatforms || [];
      if (configs.length === 0) {
        toast.error(
          i18n._(
            msg`No platform configurations found. Please create one first.`,
          ),
        );
        return;
      }
      setValidPlatformConfigs(configs);
      setStage(2);
    } finally {
      setDetectingPlatform(false);
    }
  };

  const availablePlatforms =
    validPlatformConfigs.length > 0 ? validPlatformConfigs : allPlatforms || [];

  const isLoading = platformsLoading || templatesLoading || enginesLoading;

  if (isLoading) {
    return (
      <div className="flex justify-center p-8">
        <Loader2 className="h-6 w-6 animate-spin text-muted-foreground" />
      </div>
    );
  }

  // Stage 1: Initial URL/Name Input (Wizard Style)
  if (stage === 1) {
    return (
      <div className="max-w-xl mx-auto mt-10">
        <Form {...form}>
          <form className="space-y-6">
            <div className="animate-in fade-in slide-in-from-right-4 duration-300">
              <StreamerMetaForm
                form={form}
                title={title}
                onAutofillName={handleAutofillName}
                isAutofilling={isAutofilling}
              >
                <div className="flex justify-end pt-4 gap-2">
                  <Button
                    variant="ghost"
                    type="button"
                    onClick={() => navigate({ to: '/streamers' })}
                  >
                    <Undo2 className="mr-2 h-4 w-4" /> <Trans>Cancel</Trans>
                  </Button>
                  <Button
                    type="button"
                    onClick={handleNext}
                    disabled={detectingPlatform}
                    className="min-w-[120px]"
                  >
                    {detectingPlatform ? (
                      <>
                        <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                        <Trans>Checking...</Trans>
                      </>
                    ) : (
                      <>
                        <Trans>Next</Trans>{' '}
                        <ArrowRight className="ml-2 h-4 w-4" />
                      </>
                    )}
                  </Button>
                </div>
              </StreamerMetaForm>
            </div>
          </form>
        </Form>
      </div>
    );
  }

  // Stage 2: Full Configuration (Config Only)
  return (
    <div className="min-h-screen pb-20 pt-4">
      <div className="max-w-6xl mx-auto p-4 md:p-6 relative">
        <Form {...form}>
          <form className="space-y-6">
            <div className="w-full">
              {/* Back Button for Stage 2 */}
              <Button
                variant="ghost"
                type="button"
                onClick={() => setStage(1)}
                className="mb-4"
              >
                <ArrowLeft className="mr-2 h-4 w-4" />{' '}
                <Trans>Back to Details</Trans>
              </Button>

              <StreamerConfigForm
                form={form}
                availablePlatforms={availablePlatforms}
                templates={templates}
                detectedPlatform={detectedPlatform}
                engines={engines}
              />
            </div>

            <StreamerSaveFab
              isSaving={isSubmitting}
              alwaysVisible={true}
              onSubmit={form.handleSubmit((data) => {
                // Pass data as-is - server function handles JSON serialization
                onSubmit(data);
              })}
            />
          </form>
        </Form>
      </div>
    </div>
  );
}
