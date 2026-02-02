import { createFileRoute, useNavigate } from '@tanstack/react-router';
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import { getTemplate, updateTemplate } from '@/server/functions';
import { toast } from 'sonner';
import { msg } from '@lingui/core/macro';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import {
  TemplateEditor,
  TemplateFormValues,
} from '@/components/config/templates/template-editor';
import { Skeleton } from '@/components/ui/skeleton';

export const Route = createFileRoute(
  '/_authed/_dashboard/config/templates/$templateId',
)({
  component: EditTemplatePage,
});

function EditTemplatePage() {
  const { templateId } = Route.useParams();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const { i18n } = useLingui();

  const {
    data: template,
    isLoading,
    error,
  } = useQuery({
    queryKey: ['template', templateId],
    queryFn: () => getTemplate({ data: templateId }),
  });

  const updateMutation = useMutation({
    mutationFn: (data: TemplateFormValues) =>
      updateTemplate({ data: { id: templateId, data } }),
    onSuccess: () => {
      toast.success(i18n._(msg`Template updated successfully`));
      void queryClient.invalidateQueries({ queryKey: ['templates'] });
      void queryClient.invalidateQueries({
        queryKey: ['template', templateId],
      });
      void navigate({ to: '/config/templates' });
    },
    onError: (error) =>
      toast.error(i18n._(msg`Failed to update template: ${error.message}`)),
  });

  const onSubmit = (data: TemplateFormValues) => {
    updateMutation.mutate(data);
  };

  if (isLoading) {
    return (
      <div className="space-y-6">
        <div className="flex gap-2 overflow-x-auto no-scrollbar -mx-3 px-3">
          {[1, 2, 3, 4, 5, 6, 7].map((i) => (
            <Skeleton key={i} className="h-10 w-32 shrink-0 rounded-lg" />
          ))}
        </div>
        <div className="space-y-4">
          <Skeleton className="h-24 w-full rounded-xl" />
          <Skeleton className="h-48 w-full rounded-xl" />
          <Skeleton className="h-48 w-full rounded-xl" />
        </div>
      </div>
    );
  }

  if (error || !template) {
    return (
      <div className="flex flex-col items-center justify-center p-20 text-center">
        <h3 className="text-xl font-bold text-destructive">
          <Trans>Error loading template</Trans>
        </h3>
        <p className="text-muted-foreground mt-2">
          {error?.message || i18n._(msg`Template not found`)}
        </p>
      </div>
    );
  }

  return (
    <TemplateEditor
      template={template}
      onSubmit={onSubmit}
      isSubmitting={updateMutation.isPending}
      mode="edit"
    />
  );
}
