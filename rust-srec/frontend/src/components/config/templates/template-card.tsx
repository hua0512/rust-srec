import { queryKeys } from '@/api/query-keys';
import { CardContent } from '@/components/ui/card';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog';
import { Trans } from '@lingui/react/macro';
import { msg, plural, t } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { useState } from 'react';
import { Link } from '@tanstack/react-router';
import { useQueryClient, useMutation } from '@tanstack/react-query';
import { toast } from 'sonner';
import { deleteTemplate } from '@/server/functions';
import {
  LayoutTemplate,
  Filter,
  Edit,
  Trash2,
  HardDrive,
  CopyPlus,
  FileType,
  Users,
} from 'lucide-react';
import z from 'zod';
import { TemplateSchema } from '@/api/schemas';
import { formatDate } from '@/lib/datetime';
import { errorMessage, templateInUseCount } from '@/lib/api-error';
import { templatesQueryOptions } from '@/api/templates';
import { buttonVariants } from '@/components/ui/button';
import { CardField } from '@/components/shared/card-field';
import {
  CardHeaderRow,
  CardIconTile,
  CardMenuButton,
  CardMetaFooter,
  DecoratedCard,
} from '@/components/shared/decorated-card';

interface TemplateCardProps {
  template: z.infer<typeof TemplateSchema>;
  onEdit: () => void;
  onClone?: () => void;
}

export function TemplateCard({ template, onEdit, onClone }: TemplateCardProps) {
  const [showDeleteAlert, setShowDeleteAlert] = useState(false);
  // Streamers that block the delete, from a fresh read or a refusal.
  const [blockingCount, setBlockingCount] = useState(0);
  // The backend refused because a streamer using it is still being removed.
  const [beingRemoved, setBeingRemoved] = useState(false);
  const queryClient = useQueryClient();
  const { i18n } = useLingui();
  const usageCount = template.usage_count ?? 0;

  const deleteMutation = useMutation({
    mutationFn: () => deleteTemplate({ data: template.id }),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.templates.all });
      toast.success(i18n._(msg`Deleted template "${template.name}"`));
      setShowDeleteAlert(false);
    },
    onError: (error) => {
      const inUse = templateInUseCount(error);
      if (inUse === undefined) {
        toast.error(i18n._(msg`Failed to delete template: ${error.message}`));
        return;
      }
      // Streamers still use it, or one using it is still being removed.
      setBlockingCount(inUse);
      setBeingRemoved(inUse === 0);
      setShowDeleteAlert(true);
      void queryClient.invalidateQueries({ queryKey: queryKeys.templates.all });
    },
  });

  // The cached count may be stale, so a nonzero one is refetched before it
  // blocks the delete; a stale zero still gets the backend's refusal.
  const openDeleteAlert = async () => {
    let count = usageCount;
    if (count > 0) {
      try {
        const templates = await queryClient.fetchQuery({
          ...templatesQueryOptions,
          staleTime: 0,
        });
        count =
          templates.find((item) => item.id === template.id)?.usage_count ?? 0;
      } catch (error) {
        toast.error(errorMessage(error, i18n._(msg`Failed to load templates`)));
        return;
      }
    }
    setBlockingCount(count);
    setBeingRemoved(false);
    setShowDeleteAlert(true);
  };
  const name = template.name;
  const inUseDescription = t(i18n)`${plural(blockingCount, {
    one: `"${name}" is used by # streamer. Change its template or delete it before deleting this template.`,
    other: `"${name}" is used by # streamers. Change their template or delete them before deleting this template.`,
  })}`;
  const streamersLabel = t(
    i18n,
  )`${plural(usageCount, { one: '# streamer', other: '# streamers' })}`;

  return (
    <>
      <DecoratedCard accent="purple">
        <CardHeaderRow
          icon={
            <CardIconTile
              icon={LayoutTemplate}
              className="bg-purple-500/10 text-purple-500"
            />
          }
          title={template.name}
          subtitle={
            template.stream_selection_config && (
              <div className="flex items-center gap-1 text-blue-600 dark:text-blue-400">
                <Filter className="w-3 h-3" />
                <span>
                  <Trans>Filters</Trans>
                </span>
              </div>
            )
          }
          actions={
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <CardMenuButton />
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end" className="w-48">
                <DropdownMenuItem onClick={onEdit}>
                  <Edit className="mr-2 h-4 w-4" /> <Trans>Edit</Trans>
                </DropdownMenuItem>
                {onClone && (
                  <DropdownMenuItem onClick={onClone}>
                    <CopyPlus className="mr-2 h-4 w-4" /> <Trans>Clone</Trans>
                  </DropdownMenuItem>
                )}
                <DropdownMenuSeparator />
                <DropdownMenuItem
                  onClick={() => void openDeleteAlert()}
                  className="text-destructive focus:text-destructive"
                >
                  <Trash2 className="mr-2 h-4 w-4" /> <Trans>Delete</Trans>
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          }
        />

        <CardContent className="relative pb-4 flex-1 z-10">
          <div className="grid grid-cols-2 gap-2">
            <CardField
              icon={HardDrive}
              label={<Trans>Engine</Trans>}
              value={
                template.download_engine || (
                  <span className="opacity-50">
                    <Trans>default</Trans>
                  </span>
                )
              }
              mono
            />
            <CardField
              icon={FileType}
              label={<Trans>Format</Trans>}
              value={
                template.output_file_format || (
                  <span className="opacity-50">
                    <Trans>default</Trans>
                  </span>
                )
              }
              mono
            />
            <CardField
              className="col-span-2"
              icon={Users}
              label={<Trans>Used by</Trans>}
              value={
                usageCount > 0 ? (
                  <Link
                    to="/streamers"
                    search={{ template: template.id }}
                    className="hover:underline"
                  >
                    {streamersLabel}
                  </Link>
                ) : (
                  <span className="opacity-50">
                    <Trans>No streamers</Trans>
                  </span>
                )
              }
            />
            {template.output_folder && (
              <CardField
                className="col-span-2"
                label={<Trans>Output</Trans>}
                value={template.output_folder}
                mono
              />
            )}
          </div>
        </CardContent>

        <CardMetaFooter>
          <span className="opacity-50">
            {formatDate(i18n.locale, new Date(template.updated_at || 0))}
          </span>
          <span className="font-mono opacity-50">
            #{template.id.slice(0, 8)}
          </span>
        </CardMetaFooter>
      </DecoratedCard>

      <AlertDialog open={showDeleteAlert} onOpenChange={setShowDeleteAlert}>
        <AlertDialogContent>
          {blockingCount > 0 ? (
            <>
              <AlertDialogHeader>
                <AlertDialogTitle>
                  <Trans>Template in use</Trans>
                </AlertDialogTitle>
                <AlertDialogDescription>
                  {inUseDescription}
                </AlertDialogDescription>
              </AlertDialogHeader>
              <AlertDialogFooter>
                <AlertDialogCancel>
                  <Trans>Close</Trans>
                </AlertDialogCancel>
                <Link
                  to="/streamers"
                  search={{ template: template.id }}
                  className={buttonVariants()}
                >
                  <Trans>Show streamers</Trans>
                </Link>
              </AlertDialogFooter>
            </>
          ) : (
            <>
              <AlertDialogHeader>
                <AlertDialogTitle>
                  <Trans>Delete Template</Trans>
                </AlertDialogTitle>
                <AlertDialogDescription>
                  {beingRemoved ? (
                    <Trans>
                      A streamer that used "{template.name}" is still being
                      removed. Try again in a moment.
                    </Trans>
                  ) : (
                    <Trans>
                      Are you sure you want to delete the template "
                      {template.name}"? This action cannot be undone.
                    </Trans>
                  )}
                </AlertDialogDescription>
              </AlertDialogHeader>
              <AlertDialogFooter>
                <AlertDialogCancel>
                  <Trans>Cancel</Trans>
                </AlertDialogCancel>
                <AlertDialogAction
                  onClick={() => deleteMutation.mutate()}
                  className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
                >
                  {beingRemoved ? (
                    <Trans>Try Again</Trans>
                  ) : (
                    <Trans>Delete</Trans>
                  )}
                </AlertDialogAction>
              </AlertDialogFooter>
            </>
          )}
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}
