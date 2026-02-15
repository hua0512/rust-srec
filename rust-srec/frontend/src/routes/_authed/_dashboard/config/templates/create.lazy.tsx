import { createLazyFileRoute, useNavigate } from '@tanstack/react-router';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { createTemplate } from '@/server/functions';
import { toast } from 'sonner';
import { useLingui } from '@lingui/react';
import { msg } from '@lingui/core/macro';
import {
  TemplateEditor,
  TemplateFormValues,
} from '@/components/config/templates/template-editor';

export const Route = createLazyFileRoute(
  '/_authed/_dashboard/config/templates/create',
)({
  component: CreateTemplatePage,
});

function CreateTemplatePage() {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const { i18n } = useLingui();

  const createMutation = useMutation({
    mutationFn: (data: TemplateFormValues) => {
      if (!data.name) throw new Error('Name is required');
      return createTemplate({ data: data });
    },
    onSuccess: () => {
      toast.success(i18n._(msg`Template created successfully`));
      void queryClient.invalidateQueries({ queryKey: ['templates'] });
      void navigate({ to: '/config/templates' });
    },
    onError: (error) =>
      toast.error(i18n._(msg`Failed to create template: ${error.message}`)),
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
