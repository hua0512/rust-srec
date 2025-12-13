import { useEffect, useState } from 'react';
import { Card, CardContent, CardFooter, CardHeader, CardTitle } from '@/components/ui/card';
import { Button } from '@/components/ui/button';
import { Settings2, Trash2, Cpu, CheckCircle2, XCircle, Loader2, Plus } from 'lucide-react';
import { EngineConfigSchema } from '@/api/schemas';
import { testEngine, deleteEngine } from '@/server/functions';
import { z } from 'zod';
import { EditEngineDialog } from './edit-engine-dialog';
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
} from "@/components/ui/alert-dialog";
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { toast } from 'sonner';
import { Trans } from '@lingui/react/macro';

interface EngineCardProps {
    engine: z.infer<typeof EngineConfigSchema>;
}

export function EngineCard({ engine }: EngineCardProps) {
    const queryClient = useQueryClient();
    const [status, setStatus] = useState<'loading' | 'available' | 'unavailable' | 'error'>('loading');
    const [version, setVersion] = useState<string | null>(null);

    useEffect(() => {
        let mounted = true;
        const checkAvailability = async () => {
            setStatus('loading');
            try {
                const result = await testEngine({ data: engine.id });
                if (mounted) {
                    setStatus(result.available ? 'available' : 'unavailable');
                    setVersion(result.version);
                }
            } catch (error) {
                if (mounted) {
                    setStatus('error');
                    console.error('Failed to check engine availability:', error);
                }
            }
        };

        checkAvailability();

        return () => {
            mounted = false;
        };
    }, [engine.id, engine.config]); // Re-check if config changes

    const deleteMutation = useMutation({
        mutationFn: (id: string) => deleteEngine({ data: id }),
        onSuccess: () => {
            queryClient.invalidateQueries({ queryKey: ['engines'] });
            toast.success('Engine configuration deleted');
        },
        onError: (error: Error) => {
            toast.error(`Failed to delete engine: ${error.message}`);
        }
    });

    return (
        <Card className="flex flex-col h-full border-border/50 bg-card/50 backdrop-blur-sm shadow-sm hover:shadow-md transition-all duration-300 hover:border-primary/20 group relative overflow-hidden">
            {/* Hover Glow Effect */}
            <div className="absolute -inset-0.5 bg-gradient-to-br from-primary/5 to-transparent opacity-0 group-hover:opacity-100 blur-2xl transition-opacity duration-500 pointer-events-none" />

            <CardHeader className="flex flex-row items-center gap-4 pb-2 space-y-0 relative z-10 pt-4 px-4">
                <div className="p-3 rounded-xl ring-1 ring-inset ring-black/5 dark:ring-white/5 transition-transform duration-500 group-hover:scale-105 bg-primary/10 text-primary">
                    <Cpu className="w-5 h-5" />
                </div>
                <div className="flex-1 min-w-0 space-y-1">
                    <CardTitle className="text-base font-semibold tracking-tight text-foreground/90 group-hover:text-primary transition-colors duration-300">
                        {engine.name}
                    </CardTitle>
                    <div className="text-xs text-muted-foreground font-mono">
                        {engine.engine_type}
                    </div>
                </div>
            </CardHeader>
            <CardContent className="flex-grow space-y-4 relative z-10 px-4 py-2">
                <div className="flex items-center space-x-2 text-sm bg-muted/30 p-2 rounded-lg border border-border/50">
                    <span className="font-medium text-xs uppercase tracking-wider opacity-60"><Trans>Status</Trans></span>
                    <div className="ml-auto font-medium">
                        {status === 'loading' && (
                            <div className="flex items-center text-muted-foreground text-xs">
                                <Loader2 className="w-3 h-3 mr-1.5 animate-spin" />
                                <Trans>Checking...</Trans>
                            </div>
                        )}
                        {status === 'available' && (
                            <div className="flex items-center text-green-600 dark:text-green-400 text-xs bg-green-500/10 px-2 py-0.5 rounded-full border border-green-500/10">
                                <CheckCircle2 className="w-3 h-3 mr-1.5" />
                                <Trans>Available</Trans>
                            </div>
                        )}
                        {status === 'unavailable' && (
                            <div className="flex items-center text-red-600 dark:text-red-400 text-xs bg-red-500/10 px-2 py-0.5 rounded-full border border-red-500/10">
                                <XCircle className="w-3 h-3 mr-1.5" />
                                <Trans>Unavailable</Trans>
                            </div>
                        )}
                        {status === 'error' && (
                            <div className="flex items-center text-yellow-600 dark:text-yellow-400 text-xs bg-yellow-500/10 px-2 py-0.5 rounded-full border border-yellow-500/10">
                                <XCircle className="w-3 h-3 mr-1.5" />
                                <Trans>Error</Trans>
                            </div>
                        )}
                    </div>
                </div>

                {version && (
                    <div className="text-xs bg-muted/40 p-2 rounded-md font-mono border border-border/50 text-muted-foreground truncate" title={version}>
                        {version}
                    </div>
                )}
            </CardContent>
            <CardFooter className="flex justify-between gap-2 relative z-10 px-4 pb-4 pt-0 mt-auto">
                <EditEngineDialog
                    engine={engine}
                    trigger={
                        <Button variant="ghost" className="flex-1 bg-primary/10 hover:bg-primary/15 text-primary border border-primary/10 shadow-none hover:shadow-sm h-8 text-xs">
                            <Settings2 className="w-3.5 h-3.5 mr-2" />
                            <Trans>Configure</Trans>
                        </Button>
                    }
                />
                <AlertDialog>
                    <AlertDialogTrigger asChild>
                        <Button variant="ghost" size="icon" className="h-8 w-8 text-muted-foreground hover:text-red-600 hover:bg-red-50 dark:hover:bg-red-950/20 transition-colors">
                            <Trash2 className="w-4 h-4" />
                        </Button>
                    </AlertDialogTrigger>
                    <AlertDialogContent>
                        <AlertDialogHeader>
                            <AlertDialogTitle><Trans>Are you sure?</Trans></AlertDialogTitle>
                            <AlertDialogDescription>
                                <Trans>This action cannot be undone. This will permanently delete the engine configuration "{engine.name}".</Trans>
                            </AlertDialogDescription>
                        </AlertDialogHeader>
                        <AlertDialogFooter>
                            <AlertDialogCancel><Trans>Cancel</Trans></AlertDialogCancel>
                            <AlertDialogAction onClick={() => deleteMutation.mutate(engine.id)} className="bg-red-600 hover:bg-red-700 text-white"><Trans>Delete</Trans></AlertDialogAction>
                        </AlertDialogFooter>
                    </AlertDialogContent>
                </AlertDialog>
            </CardFooter>
        </Card>
    );
}

export function CreateEngineCard() {
    return (
        <Card className="flex flex-col h-full border-dashed border-2 border-muted hover:border-primary/50 bg-transparent hover:bg-muted/5 transition-all duration-300 group cursor-pointer justify-center items-center min-h-[200px]">
            <CardContent className="flex flex-col items-center justify-center p-6 space-y-4">
                <div className="p-4 rounded-full bg-muted group-hover:bg-primary/10 transition-colors duration-300">
                    <Plus className="w-8 h-8 text-muted-foreground group-hover:text-primary transition-colors" />
                </div>
                <div className="space-y-1 text-center">
                    <h3 className="font-semibold text-lg tracking-tight group-hover:text-primary transition-colors"><Trans>Add New Engine</Trans></h3>
                    <p className="text-sm text-muted-foreground"><Trans>Configure a new download engine</Trans></p>
                </div>
                <EditEngineDialog
                    trigger={
                        <Button className="mt-4" variant="default">
                            <Trans>Create Engine</Trans>
                        </Button>
                    }
                />
            </CardContent>
        </Card>
    );
}
