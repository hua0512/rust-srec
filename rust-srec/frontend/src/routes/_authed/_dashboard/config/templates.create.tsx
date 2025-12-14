import { createFileRoute, useNavigate } from '@tanstack/react-router';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { createTemplate } from '@/server/functions';
import { toast } from 'sonner';
import { t } from '@lingui/core/macro';
import {
  TemplateEditor,
  TemplateFormValues,
} from '@/components/config/templates/template-editor';

export const Route = createFileRoute(
  '/_authed/_dashboard/config/templates/create',
)({
  component: CreateTemplatePage,
});

function CreateTemplatePage() {
  const navigate = useNavigate();
  const queryClient = useQueryClient();

  const createMutation = useMutation({
    mutationFn: (data: TemplateFormValues) => {
      if (!data.name) throw new Error('Name is required');
      return createTemplate({ data: data as any });
    },
    onSuccess: () => {
      toast.success(t`Template created successfully`);
      queryClient.invalidateQueries({ queryKey: ['templates'] });
      navigate({ to: '/config/templates' });
    },
    onError: (error) =>
      toast.error(t`Failed to create template: ${error.message}`),
  });

  const onSubmit = (data: TemplateFormValues) => {
    createMutation.mutate(data);
  };

  return (
    <TemplateEditor
      onSubmit={onSubmit}
      isSubmitting={createMutation.isPending}
      mode="create"
    />
  );
}
