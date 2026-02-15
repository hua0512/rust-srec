import { createLazyFileRoute, useNavigate } from '@tanstack/react-router';
import { createStreamer } from '@/server/functions';
import { useMutation } from '@tanstack/react-query';
import { toast } from 'sonner';
import { msg } from '@lingui/core/macro';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { StreamerForm } from '@/components/streamers/streamer-form';
import { StreamerFormValues } from '@/api/schemas';

export const Route = createLazyFileRoute('/_authed/_dashboard/streamers/new')({
  component: CreateStreamerPage,
});

function CreateStreamerPage() {
  const navigate = useNavigate();
  const { i18n } = useLingui();

  const createMutation = useMutation({
    mutationFn: (data: any) => createStreamer({ data }),
    onSuccess: () => {
      toast.success(i18n._(msg`Streamer created successfully`));
      void navigate({ to: '/streamers' });
    },
    onError: (error: any) => {
      toast.error(error.message || i18n._(msg`Failed to create streamer`));
    },
  });

  const onSubmit = (data: StreamerFormValues) => {
    const payload = {
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
    createMutation.mutate(payload);
  };

  return (
    <div className="max-w-6xl mx-auto p-4 md:p-8 space-y-6 animate-in fade-in slide-in-from-bottom-4 duration-500">
      <StreamerForm
        onSubmit={onSubmit}
        isSubmitting={createMutation.isPending}
        title={<Trans>Add New Streamer</Trans>}
        description={
          <Trans>Configure a new streamer to monitor and record.</Trans>
        }
        submitLabel={<Trans>Create Streamer</Trans>}
      />
    </div>
  );
}
