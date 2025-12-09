import { createFileRoute } from '@tanstack/react-router';
import { useQuery } from '@tanstack/react-query';
import { listTemplates } from '@/server/functions';
import { Trans } from '@lingui/react/macro';
import { Skeleton } from '../../../../components/ui/skeleton';
import { TemplateCard } from '../../../../components/config/templates/template-card';
import { EditTemplateDialog } from '../../../../components/config/templates/edit-template-dialog';
import { AlertCircle, Plus } from 'lucide-react';
import { Alert, AlertDescription, AlertTitle } from '../../../../components/ui/alert';

export const Route = createFileRoute('/_authed/_dashboard/config/templates')({
  component: TemplatesConfigPage,
});

function TemplatesConfigPage() {
  const { data: templates, isLoading, error } = useQuery({
    queryKey: ['templates'],
    queryFn: () => listTemplates(),
  });

  if (error) {
    return (
      <Alert variant="destructive">
        <AlertCircle className="h-4 w-4" />
        <AlertTitle>Error</AlertTitle>
        <AlertDescription>
          Failed to load templates: {(error as baseError).message}
        </AlertDescription>
      </Alert>
    );
  }

  return (
    <div className="space-y-6 animate-in fade-in duration-500">
      {isLoading ? (
        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 gap-4">
          {Array.from({ length: 4 }).map((_, i) => (
            <Skeleton key={i} className="h-[200px] w-full rounded-xl" />
          ))}
        </div>
      ) : (
        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 gap-4">
          <EditTemplateDialog
            trigger={
              <button className="flex flex-col items-center justify-center h-full min-h-[200px] border-2 border-dashed rounded-xl hover:border-primary/50 hover:bg-muted/50 transition-all cursor-pointer group space-y-4 text-muted-foreground hover:text-foreground">
                <div className="p-4 rounded-full bg-primary/10 group-hover:bg-primary/20 transition-colors">
                  <Plus className="w-8 h-8 text-primary" />
                </div>
                <p className="font-medium text-lg">
                  <Trans>Create Template</Trans>
                </p>
              </button>
            }
          />
          {templates?.map((template) => (
            <TemplateCard key={template.id} template={template} />
          ))}
        </div>
      )}
    </div>
  );
}

interface baseError {
  message: string;
}
