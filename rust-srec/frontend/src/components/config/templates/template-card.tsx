import {
    Card,
    CardContent,
    CardFooter,
    CardHeader,
    CardTitle,
} from '@/components/ui/card';
import { Button } from '@/components/ui/button';
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
} from "@/components/ui/alert-dialog";
import { Trans } from '@lingui/react/macro';
import { useState } from 'react';
import { useQueryClient, useMutation } from '@tanstack/react-query';
import { toast } from 'sonner';
import { deleteTemplate } from '@/server/functions';
import { LayoutTemplate, Cookie, Filter, MoreHorizontal, Edit, Trash2, HardDrive, FileType } from 'lucide-react';
import z from 'zod';
import { TemplateSchema } from '@/api/schemas';

interface TemplateCardProps {
    template: z.infer<typeof TemplateSchema>;
    onEdit: () => void;
}

export function TemplateCard({ template, onEdit }: TemplateCardProps) {
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

    return (
        <>
            <Card className="relative h-full flex flex-col transition-all duration-500 hover:-translate-y-1 hover:shadow-2xl hover:shadow-primary/10 group overflow-hidden bg-gradient-to-br from-background/80 to-background/40 backdrop-blur-xl border-border/40 hover:border-primary/20">
                {/* Top gradient line */}
                <div className="absolute inset-x-0 top-0 h-0.5 bg-gradient-to-r from-transparent via-purple-500/40 to-transparent opacity-0 group-hover:opacity-100 transition-opacity duration-700" />

                {/* Hover Glow Effect */}
                <div className="absolute -inset-0.5 bg-gradient-to-br from-purple-500/5 to-transparent opacity-0 group-hover:opacity-100 blur-2xl transition-opacity duration-500 pointer-events-none" />

                <CardHeader className="relative flex flex-row items-center gap-4 pb-2 space-y-0 z-10">
                    <div className="p-3 rounded-2xl bg-purple-500/10 text-purple-500 ring-1 ring-inset ring-black/5 dark:ring-white/5 transition-transform duration-500 group-hover:scale-110 group-hover:rotate-3">
                        <LayoutTemplate className="h-5 w-5" />
                    </div>
                    <div className="flex-1 min-w-0 space-y-1">
                        <CardTitle className="text-base font-medium truncate tracking-tight text-foreground/90 group-hover:text-primary transition-colors duration-300">
                            {template.name}
                        </CardTitle>
                        <div className="flex items-center gap-2">
                            {template.cookies && (
                                <div className="flex items-center gap-1 text-[10px] uppercase tracking-wider font-semibold text-orange-600 dark:text-orange-400">
                                    <Cookie className="w-3 h-3" />
                                    <span>Cookies</span>
                                </div>
                            )}
                            {template.stream_selection_config && (
                                <div className="flex items-center gap-1 text-[10px] uppercase tracking-wider font-semibold text-blue-600 dark:text-blue-400">
                                    <Filter className="w-3 h-3" />
                                    <span>Filters</span>
                                </div>
                            )}
                        </div>
                    </div>
                    <DropdownMenu>
                        <DropdownMenuTrigger asChild>
                            <Button variant="ghost" size="icon" className="h-8 w-8 -mr-2 text-muted-foreground/40 hover:text-foreground transition-colors">
                                <MoreHorizontal className="h-4 w-4" />
                                <span className="sr-only"><Trans>Open menu</Trans></span>
                            </Button>
                        </DropdownMenuTrigger>
                        <DropdownMenuContent align="end" className="w-48">
                            <DropdownMenuItem onClick={onEdit}>
                                <Edit className="mr-2 h-4 w-4" /> <Trans>Edit</Trans>
                            </DropdownMenuItem>
                            <DropdownMenuSeparator />
                            <DropdownMenuItem
                                onClick={() => setShowDeleteAlert(true)}
                                className="text-destructive focus:text-destructive"
                            >
                                <Trash2 className="mr-2 h-4 w-4" /> <Trans>Delete</Trans>
                            </DropdownMenuItem>
                        </DropdownMenuContent>
                    </DropdownMenu>
                </CardHeader>

                <CardContent className="relative pb-4 flex-1 z-10">
                    <div className="grid grid-cols-2 gap-2">
                        <div className="flex flex-col gap-0.5 bg-muted/30 rounded-md px-2 py-1.5 border border-transparent group-hover:border-primary/5 transition-colors">
                            <span className="text-[9px] uppercase tracking-wider opacity-50 flex items-center gap-1">
                                <HardDrive className="w-3 h-3" />
                                <Trans>Engine</Trans>
                            </span>
                            <span className="text-[11px] font-medium truncate text-foreground/80 font-mono">
                                {template.download_engine || <span className="opacity-50">default</span>}
                            </span>
                        </div>
                        <div className="flex flex-col gap-0.5 bg-muted/30 rounded-md px-2 py-1.5 border border-transparent group-hover:border-primary/5 transition-colors">
                            <span className="text-[9px] uppercase tracking-wider opacity-50 flex items-center gap-1">
                                <FileType className="w-3 h-3" />
                                <Trans>Format</Trans>
                            </span>
                            <span className="text-[11px] font-medium truncate text-foreground/80 font-mono">
                                {template.output_file_format || <span className="opacity-50">default</span>}
                            </span>
                        </div>
                        {template.output_folder && (
                            <div className="col-span-2 flex flex-col gap-0.5 bg-muted/30 rounded-md px-2 py-1.5 border border-transparent group-hover:border-primary/5 transition-colors">
                                <span className="text-[9px] uppercase tracking-wider opacity-50">
                                    <Trans>Output</Trans>
                                </span>
                                <span className="text-[11px] font-medium truncate text-foreground/80 font-mono">
                                    {template.output_folder}
                                </span>
                            </div>
                        )}
                    </div>
                </CardContent>

                <CardFooter className="relative pt-0 text-[10px] text-muted-foreground flex justify-between items-center z-10 border-t border-border/20 mt-auto px-6 py-3 bg-muted/5">
                    <span className="opacity-50">
                        {new Date(template.updated_at).toLocaleDateString()}
                    </span>
                    <span className="font-mono opacity-50">#{template.id.slice(0, 8)}</span>
                </CardFooter>
            </Card>

            <AlertDialog open={showDeleteAlert} onOpenChange={setShowDeleteAlert}>
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
