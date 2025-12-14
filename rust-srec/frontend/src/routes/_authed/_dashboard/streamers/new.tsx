import { createFileRoute, useNavigate } from '@tanstack/react-router';
import { createStreamer } from '@/server/functions';
import { useMutation } from '@tanstack/react-query';
import { toast } from 'sonner';
import { t } from '@lingui/core/macro';
import { Trans } from '@lingui/react/macro';
import { StreamerForm } from '../../../../components/streamers/streamer-form';
import { CreateStreamerSchema } from '../../../../api/schemas';
import { z } from 'zod';

export const Route = createFileRoute('/_authed/_dashboard/streamers/new')({
  component: CreateStreamerPage,
});

function CreateStreamerPage() {
  const navigate = useNavigate();

  const createMutation = useMutation({
    mutationFn: (data: any) => createStreamer({ data }),
    onSuccess: () => {
      toast.success(t`Streamer created successfully`);
      navigate({ to: '/streamers' });
    },
    onError: (error: any) => {
      toast.error(error.message || t`Failed to create streamer`);
    },
  });

  const onSubmit = (data: z.infer<typeof CreateStreamerSchema>) => {
    // If "none" is selected, we should send undefined or null, but react-hook-form might keep "none" string.
    // The Schema expects optional string.
    // Let's normalize data before sending
    const payload = {
      ...data,
      platform_config_id:
        data.platform_config_id === 'none'
          ? undefined
          : data.platform_config_id,
      template_id: data.template_id === 'none' ? undefined : data.template_id,
    };
    createMutation.mutate(payload);
  };

  return (
    <div className="max-w-3xl mx-auto p-4 md:p-8 space-y-6 animate-in fade-in slide-in-from-bottom-4 duration-500">
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
