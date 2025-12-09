import { Card, CardContent, CardHeader, CardTitle } from '../../ui/card';
import { Button } from '../../ui/button';
import { Trans } from '@lingui/react/macro';
import { useState } from 'react';
import { useQueryClient, useMutation } from '@tanstack/react-query';
import { toast } from 'sonner';
import { deleteTemplate } from '@/server/functions';
import { Settings, Cookie, Filter, LayoutTemplate, Trash2 } from 'lucide-react';
import {
    AlertDialog,
    AlertDialogAction,
    AlertDialogCancel,
    AlertDialogContent,
    AlertDialogDescription,
    AlertDialogFooter,
    AlertDialogHeader,
    AlertDialogTitle,
    AlertDialogTrigger,
} from "../../ui/alert-dialog";
import { EditTemplateDialog } from './edit-template-dialog';
import z from 'zod';
import { TemplateSchema } from '@/api/schemas';

interface TemplateCardProps {
    template: z.infer<typeof TemplateSchema>;
}

export function TemplateCard({ template }: TemplateCardProps) {
    const [showDeleteAlert, setShowDeleteAlert] = useState(false);
    const queryClient = useQueryClient();

    const deleteMutation = useMutation({
        mutationFn: () => deleteTemplate({ data: template.id }),
        onSuccess: () => {
            queryClient.invalidateQueries({ queryKey: ['templates'] });
            toast.success(`Deleted template "${template.name}"`);
            setShowDeleteAlert(false);
        },
        onError: (error) => {
            toast.error(`Failed to delete template: ${error.message}`);
        },
    });

    const handleDelete = () => {
        deleteMutation.mutate();
    };
    return (
        <Card className="group overflow-hidden hover:shadow-lg transition-all duration-300 border-muted/60 flex flex-col">
            <CardHeader className="pb-3 border-b bg-muted/20">
                <div className="flex items-center justify-between">
                    <CardTitle className="flex items-center gap-2.5 text-lg">
                        <div className="p-1.5 rounded-md bg-purple-500/10 text-purple-600 dark:text-purple-400">
                            <LayoutTemplate className="w-5 h-5" />
                        </div>
                        {template.name}
                    </CardTitle>
                    <div className="flex gap-1">
                        {template.cookies && (
                            <div className="p-1.5 rounded-md bg-orange-500/10 text-orange-600 dark:text-orange-400" title="Cookies Set">
                                <Cookie className="w-3.5 h-3.5" />
                            </div>
                        )}
                        {template.stream_selection_config && (
                            <div className="p-1.5 rounded-md bg-blue-500/10 text-blue-600 dark:text-blue-400" title="Stream Selection Configured">
                                <Filter className="w-3.5 h-3.5" />
                            </div>
                        )}
                    </div>
                </div>
            </CardHeader>
            <CardContent className="pt-4 flex-1 grid gap-4">
                <div className="grid grid-cols-2 gap-4 text-sm">
                    <div className="space-y-1">
                        <span className="text-muted-foreground text-xs uppercase tracking-wider font-semibold"><Trans>Engine</Trans></span>
                        <div className="font-mono bg-muted/50 p-1.5 rounded text-center truncate">
                            {template.download_engine || <span className="text-muted-foreground">-</span>}
                        </div>
                    </div>
                    <div className="space-y-1">
                        <span className="text-muted-foreground text-xs uppercase tracking-wider font-semibold"><Trans>Format</Trans></span>
                        <div className="font-mono bg-muted/50 p-1.5 rounded text-center truncate">
                            {template.output_file_format || <span className="text-muted-foreground">-</span>}
                        </div>
                    </div>
                </div>

                <div className="mt-auto pt-2 flex gap-2">
                    <EditTemplateDialog
                        template={template}
                        trigger={
                            <Button variant="outline" className="flex-1 group-hover:border-primary/50 group-hover:text-primary transition-colors">
                                <Settings className="w-4 h-4 mr-2" />
                                <Trans>Configure</Trans>
                            </Button>
                        }
                    />
                    <AlertDialog open={showDeleteAlert} onOpenChange={setShowDeleteAlert}>
                        <AlertDialogTrigger asChild>
                            <Button variant="outline" size="icon" className="shrink-0 text-destructive border-destructive/30 hover:bg-destructive/10 hover:text-destructive hover:border-destructive/50">
                                <Trash2 className="w-4 h-4" />
                            </Button>
                        </AlertDialogTrigger>
                        <AlertDialogContent>
                            <AlertDialogHeader>
                                <AlertDialogTitle><Trans>Delete Template</Trans></AlertDialogTitle>
                                <AlertDialogDescription>
                                    <Trans>
                                        Are you sure you want to delete the template "{template.name}"? This action cannot be undone.
                                    </Trans>
                                </AlertDialogDescription>
                            </AlertDialogHeader>
                            <AlertDialogFooter>
                                <AlertDialogCancel><Trans>Cancel</Trans></AlertDialogCancel>
                                <AlertDialogAction onClick={handleDelete} className="bg-destructive text-destructive-foreground hover:bg-destructive/90">
                                    <Trans>Delete</Trans>
                                </AlertDialogAction>
                            </AlertDialogFooter>
                        </AlertDialogContent>
                    </AlertDialog>
                </div>

                <div className="text-xs text-muted-foreground mt-2 flex justify-end">
                    <span title={new Date(template.created_at).toLocaleString()}>
                        Updated: {new Date(template.updated_at).toLocaleDateString()}
                    </span>
                </div>
            </CardContent>
        </Card >
    );
}
