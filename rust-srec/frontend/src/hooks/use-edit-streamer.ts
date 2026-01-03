import { useEffect, useState, useCallback, useMemo } from 'react';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useNavigate } from '@tanstack/react-router';
import { toast } from 'sonner';
import { t } from '@lingui/core/macro';
import { useForm } from 'react-hook-form';
import { zodResolver } from '@hookform/resolvers/zod';
import { z } from 'zod';
import {
  UpdateStreamerSchema,
  StreamerFormSchema,
  StreamerFormValues,
} from '@/api/schemas';
import {
  updateStreamer,
  deleteFilter,
  parseUrl,
  getStreamer,
} from '@/server/functions';

interface UseEditStreamerProps {
  id: string;
  streamer: NonNullable<Awaited<ReturnType<typeof getStreamer>>>;
}

export function useEditStreamer({ id, streamer }: UseEditStreamerProps) {
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const [isAutofilling, setIsAutofilling] = useState(false);

  // Parse the specific config
  const specificConfig = useMemo(
    () =>
      typeof streamer.streamer_specific_config === 'string'
        ? JSON.parse(streamer.streamer_specific_config)
        : (streamer.streamer_specific_config ?? {}),
    [streamer.streamer_specific_config],
  );

  // Initialize form
  const form = useForm<StreamerFormValues>({
    resolver: zodResolver(StreamerFormSchema) as any,
    defaultValues: {
      name: streamer.name,
      url: streamer.url,
      enabled: streamer.enabled,
      priority: streamer.priority,
      platform_config_id: streamer.platform_config_id || '',
      template_id: streamer.template_id ?? null,
      streamer_specific_config: specificConfig,
    },
    reValidateMode: 'onBlur',
  });

  const { reset } = form;

  // Reset form when streamer data changes
  useEffect(() => {
    reset({
      name: streamer.name,
      url: streamer.url,
      enabled: streamer.enabled,
      priority: streamer.priority,
      platform_config_id: streamer.platform_config_id || '',
      template_id: streamer.template_id ?? null,
      streamer_specific_config: specificConfig,
    });
  }, [streamer, reset, specificConfig]);

  const updateMutation = useMutation({
    mutationFn: (data: z.infer<typeof UpdateStreamerSchema>) =>
      updateStreamer({ data: { id, data } }),
    onSuccess: () => {
      toast.success(t`Streamer updated successfully`);
      queryClient.invalidateQueries({ queryKey: ['streamers'] });
      queryClient.invalidateQueries({ queryKey: ['streamer', id] });
      navigate({ to: '/streamers' });
    },
    onError: (error: any) => {
      toast.error(error.message || t`Failed to update streamer`);
    },
  });

  const deleteFilterMutation = useMutation({
    mutationFn: (filterId: string) =>
      deleteFilter({ data: { streamerId: id, filterId } }),
    onSuccess: () => {
      toast.success(t`Filter deleted successfully`);
      queryClient.invalidateQueries({ queryKey: ['streamers', id, 'filters'] });
    },
    onError: (error: any) => {
      toast.error(error.message || t`Failed to delete filter`);
    },
  });

  const handleAutofillName = useCallback(async () => {
    const url = form.getValues('url');
    if (!url) return;

    const urlValid = await form.trigger('url');
    if (!urlValid) return;

    setIsAutofilling(true);
    try {
      const response = await parseUrl({ data: { url } });
      if (response.success && response.media_info?.artist) {
        form.setValue('name', response.media_info.artist, {
          shouldDirty: true,
          shouldValidate: true,
        });
        toast.success(t`Name autofilled successfully`);
      } else if (response.error) {
        toast.error(response.error);
      } else {
        toast.error(t`Failed to extract name from URL`);
      }
    } catch (error: any) {
      console.error('Failed to autofill name:', error);
      toast.error(error.message || t`Failed to autofill name`);
    } finally {
      setIsAutofilling(false);
    }
  }, [form]);

  const onSubmit = useCallback(
    (data: StreamerFormValues) => {
      const payload: z.infer<typeof UpdateStreamerSchema> = {
        ...data,
        platform_config_id:
          data.platform_config_id === 'none' || data.platform_config_id === ''
            ? undefined
            : data.platform_config_id,
        template_id:
          data.template_id === null || data.template_id === 'none'
            ? null
            : data.template_id,
        streamer_specific_config: data.streamer_specific_config ?? undefined,
      };
      updateMutation.mutate(payload);
    },
    [updateMutation],
  );

  const onInvalid = useCallback((errors: any) => {
    console.error('Form validation errors:', errors);
    toast.error(t`Please fix validation errors`);
  }, []);

  const deleteFilterCallback = useCallback(
    (filterId: string) => deleteFilterMutation.mutate(filterId),
    [deleteFilterMutation],
  );

  return useMemo(
    () => ({
      form,
      isAutofilling,
      handleAutofillName,
      onSubmit,
      onInvalid,
      isSaving: updateMutation.isPending,
      deleteFilter: deleteFilterCallback,
    }),
    [
      form,
      isAutofilling,
      handleAutofillName,
      onSubmit,
      onInvalid,
      updateMutation.isPending,
      deleteFilterCallback,
    ],
  );
}
