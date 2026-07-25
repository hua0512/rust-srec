import { useCallback, useEffect, useMemo, useState } from 'react';
import { useLingui } from '@lingui/react';
import { msg } from '@lingui/core/macro';
import { toast } from 'sonner';
import { useForm } from 'react-hook-form';
import { zodResolver } from '@hookform/resolvers/zod';
import { z } from 'zod';
import {
  CreateStreamerSchema,
  StreamerFormSchema,
  StreamerFormValues,
} from '@/api/schemas';
import { parseUrl, getStreamer } from '@/server/functions';

type Streamer = NonNullable<Awaited<ReturnType<typeof getStreamer>>>;

/**
 * Payload accepted by both endpoints.
 *
 * Shaped for create (name and url present, which validation guarantees); update takes the same
 * object because `UpdateStreamerSchema` is the partial of this one.
 */
export type StreamerPayload = z.infer<typeof CreateStreamerSchema>;

interface UseStreamerFormOptions {
  /**
   * The streamer being edited. Omitted in create mode, where the form starts from defaults.
   * Changes to this object reset the form, so a background refetch does not strand stale values.
   */
  streamer?: Streamer;
}

/**
 * The form contract shared by the create wizard and the edit page.
 *
 * Both flows bind the same `StreamerFormSchema`, parse `streamer_specific_config` the same way,
 * autofill the name from the same endpoint, and send the same payload shape. Keeping that in one
 * place is what stops the two screens from drifting apart.
 */
export function useStreamerForm({ streamer }: UseStreamerFormOptions = {}) {
  const { i18n } = useLingui();
  const [isAutofilling, setIsAutofilling] = useState(false);

  // The API may hand back `streamer_specific_config` as a JSON string or an object depending on
  // the code path; the form always works with an object.
  const specificConfig = useMemo(() => {
    const raw = streamer?.streamer_specific_config;
    if (typeof raw === 'string') return JSON.parse(raw);
    return raw ?? {};
  }, [streamer?.streamer_specific_config]);

  const defaultValues = useMemo<StreamerFormValues>(
    () => ({
      name: streamer?.name ?? '',
      url: streamer?.url ?? '',
      enabled: streamer?.enabled ?? true,
      priority: streamer?.priority ?? 'NORMAL',
      template_id: streamer?.template_id ?? null,
      streamer_specific_config: specificConfig,
    }),
    [streamer, specificConfig],
  );

  const form = useForm<StreamerFormValues>({
    resolver: zodResolver(StreamerFormSchema),
    defaultValues,
    // Validate as the user types so the wizard can gate its Next button, but re-validate on blur
    // afterwards so a half-typed URL doesn't flash an error on every keystroke.
    mode: 'onChange',
    reValidateMode: 'onBlur',
  });

  const { reset } = form;

  useEffect(() => {
    if (streamer) reset(defaultValues);
  }, [streamer, defaultValues, reset]);

  /** Trim the URL in place and validate it. Returns the trimmed URL, or null if invalid. */
  const trimAndValidateUrl = useCallback(async (): Promise<string | null> => {
    const url = form.getValues('url')?.trim();
    if (!url) return null;
    form.setValue('url', url);
    return (await form.trigger('url')) ? url : null;
  }, [form]);

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
  }, [form, i18n, trimAndValidateUrl]);

  /**
   * Normalize form values into an API payload.
   *
   * `template_id` uses the sentinel `'none'` in the select, which the API expects as `null`.
   */
  const toPayload = useCallback(
    (data: StreamerFormValues): StreamerPayload => ({
      ...data,
      template_id:
        data.template_id === null || data.template_id === 'none'
          ? null
          : data.template_id,
      streamer_specific_config: data.streamer_specific_config ?? undefined,
    }),
    [],
  );

  const onInvalid = useCallback(
    (errors: unknown) => {
      console.error('Form validation errors:', errors);
      toast.error(i18n._(msg`Please fix validation errors`));
    },
    [i18n],
  );

  return {
    form,
    isAutofilling,
    handleAutofillName,
    trimAndValidateUrl,
    toPayload,
    onInvalid,
  };
}
