
import { useEffect, useState } from 'react';
import { Card, CardContent, CardDescription, CardFooter, CardHeader, CardTitle } from '@/components/ui/card';
import { Button } from '@/components/ui/button';
import { Settings2, Trash2, Cpu, CheckCircle2, XCircle, Loader2 } from 'lucide-react';
import { EngineConfigSchema } from '@/api/schemas';
import { engineApi } from '@/api/endpoints';
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
                const result = await engineApi.test(engine.id);
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
        mutationFn: engineApi.delete,
        onSuccess: () => {
            queryClient.invalidateQueries({ queryKey: ['engines'] });
            toast.success('Engine configuration deleted');
        },
        onError: (error: Error) => {
            toast.error(`Failed to delete engine: ${error.message}`);
        }
    });

    return (
        <Card className="flex flex-col h-full">
            <CardHeader>
                <div className="flex items-center space-x-2">
                    <Cpu className="w-5 h-5 text-primary" />
                    <CardTitle className="text-xl">{engine.name}</CardTitle>
                </div>
                <CardDescription>
                    Type: <span className="font-semibold">{engine.engine_type}</span>
                </CardDescription>
            </CardHeader>
            <CardContent className="flex-grow space-y-4">
                <div className="text-sm text-muted-foreground truncate">
                    ID: {engine.id}
                </div>

                <div className="flex items-center space-x-2 text-sm">
                    <span className="font-medium">Status:</span>
                    {status === 'loading' && (
                        <div className="flex items-center text-muted-foreground">
                            <Loader2 className="w-3 h-3 mr-1 animate-spin" />
                            Checking...
                        </div>
                    )}
                    {status === 'available' && (
                        <div className="flex items-center text-green-600 dark:text-green-400">
                            <CheckCircle2 className="w-4 h-4 mr-1" />
                            Available
                        </div>
                    )}
                    {status === 'unavailable' && (
                        <div className="flex items-center text-red-600 dark:text-red-400">
                            <XCircle className="w-4 h-4 mr-1" />
                            Unavailable
                        </div>
                    )}
                    {status === 'error' && (
                        <div className="flex items-center text-yellow-600 dark:text-yellow-400">
                            <XCircle className="w-4 h-4 mr-1" />
                            Error checking
                        </div>
                    )}
                </div>

                {version && (
                    <div className="text-xs bg-muted p-2 rounded-md font-mono">
                        {version}
                    </div>
                )}
            </CardContent>
            <CardFooter className="flex justify-between gap-2">
                <EditEngineDialog
                    engine={engine}
                    trigger={
                        <Button variant="outline" className="flex-1">
                            <Settings2 className="w-4 h-4 mr-2" />
                            Configure
                        </Button>
                    }
                />
                <AlertDialog>
                    <AlertDialogTrigger asChild>
                        <Button variant="destructive" size="icon">
                            <Trash2 className="w-4 h-4" />
                        </Button>
                    </AlertDialogTrigger>
                    <AlertDialogContent>
                        <AlertDialogHeader>
                            <AlertDialogTitle>Are you sure?</AlertDialogTitle>
                            <AlertDialogDescription>
                                This action cannot be undone. This will permanently delete the engine configuration "{engine.name}".
                            </AlertDialogDescription>
                        </AlertDialogHeader>
                        <AlertDialogFooter>
                            <AlertDialogCancel>Cancel</AlertDialogCancel>
                            <AlertDialogAction onClick={() => deleteMutation.mutate(engine.id)}>Delete</AlertDialogAction>
                        </AlertDialogFooter>
                    </AlertDialogContent>
                </AlertDialog>
            </CardFooter>
        </Card>
    );
}

export function CreateEngineCard() {
    return (
        <Card className="flex flex-col h-full border-dashed">
            <CardHeader>
                <CardTitle className="text-xl">Add New Engine</CardTitle>
                <CardDescription>Configure a new download engine</CardDescription>
            </CardHeader>
            <CardContent className="flex-grow flex items-center justify-center">
                <EditEngineDialog
                    trigger={
                        <Button size="lg" className="w-full">
                            Create Engine
                        </Button>
                    }
                />
            </CardContent>
        </Card>
    );
}
