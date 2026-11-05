import { createLazyFileRoute, useNavigate } from '@tanstack/react-router';
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import { queryKeys } from '@/api/query-keys';
import { invalidateCredentialQueries } from '@/api/credential-profiles';
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
import { DetailLoadError } from '@/components/shared/detail-page';
import { SettingsCardSkeleton } from '@/components/config/settings-card-skeleton';

export const Route = createLazyFileRoute(
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
    queryKey: queryKeys.templates.detail(templateId),
    queryFn: () => getTemplate({ data: templateId }),
  });

  const updateMutation = useMutation({
    mutationFn: (data: TemplateFormValues) =>
      updateTemplate({ data: { id: templateId, data } }),
    onSuccess: () => {
      toast.success(i18n._(msg`Template updated successfully`));
      void queryClient.invalidateQueries({ queryKey: queryKeys.templates.all });
      // The form also saves an account selection per platform override.
      void invalidateCredentialQueries(queryClient);
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
        {/* `defaultTab="general"` opens on `GeneralTab`'s single card, which holds only the
            template name field. */}
        <SettingsCardSkeleton sections={[1]} />
      </div>
    );
  }

  if (error || !template) {
    return (
      <DetailLoadError
        title={<Trans>Error loading template</Trans>}
        error={error}
        notFound={<Trans>Template not found</Trans>}
        back={{
          to: '/config/templates',
          label: <Trans>Back to templates</Trans>,
        }}
      />
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
