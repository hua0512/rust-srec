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
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { useState } from 'react';
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
} from 'lucide-react';
import z from 'zod';
import { TemplateSchema } from '@/api/schemas';
import { formatDate } from '@/lib/datetime';
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
  const queryClient = useQueryClient();
  const { i18n } = useLingui();

  const deleteMutation = useMutation({
    mutationFn: () => deleteTemplate({ data: template.id }),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ['templates'] });
      toast.success(i18n._(msg`Deleted template "${template.name}"`));
      setShowDeleteAlert(false);
    },
    onError: (error) => {
      toast.error(i18n._(msg`Failed to delete template: ${error.message}`));
    },
  });

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
                <span>Filters</span>
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
                  onClick={() => setShowDeleteAlert(true)}
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
                  <span className="opacity-50">default</span>
                )
              }
              mono
            />
            <CardField
              icon={FileType}
              label={<Trans>Format</Trans>}
              value={
                template.output_file_format || (
                  <span className="opacity-50">default</span>
                )
              }
              mono
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
          <AlertDialogHeader>
            <AlertDialogTitle>
              <Trans>Delete Template</Trans>
            </AlertDialogTitle>
            <AlertDialogDescription>
              <Trans>
                Are you sure you want to delete the template "{template.name}"?
                This action cannot be undone.
              </Trans>
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
              <Trans>Delete</Trans>
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}
