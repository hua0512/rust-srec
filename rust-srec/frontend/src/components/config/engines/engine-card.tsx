import { useEffect, useState } from 'react';
import {
  Card,
  CardContent,
  CardFooter,
  CardHeader,
  CardTitle,
} from '@/components/ui/card';
import { Button } from '@/components/ui/button';
import { Badge } from '@/components/ui/badge';
import {
  Settings2,
  Trash2,
  Cpu,
  CheckCircle2,
  XCircle,
  Loader2,
  Plus,
  AlertCircle,
  PlayCircle,
} from 'lucide-react';
import { EngineConfigSchema } from '@/api/schemas';
import { testEngine, deleteEngine } from '@/server/functions';
import { z } from 'zod';
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
} from '@/components/ui/alert-dialog';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { toast } from 'sonner';
import { Trans } from '@lingui/react/macro';
import { cn } from '@/lib/utils';
import { Link } from '@tanstack/react-router';

interface EngineCardProps {
  engine: z.infer<typeof EngineConfigSchema>;
}

export function EngineCard({ engine }: EngineCardProps) {
  const queryClient = useQueryClient();
  const [status, setStatus] = useState<
    'loading' | 'available' | 'unavailable' | 'error'
  >('loading');
  const [version, setVersion] = useState<string | null>(null);

  useEffect(() => {
    let mounted = true;
    const checkAvailability = async () => {
      setStatus('loading');
      try {
        const result = await testEngine({ data: engine.id });
        if (mounted) {
          setStatus(result.available ? 'available' : 'unavailable');
          setVersion(result.version ?? 'unknown');
        }
      } catch (error) {
        if (mounted) {
          setStatus('error');
          console.error('Failed to checked engine availability:', error);
        }
      }
    };

    checkAvailability();

    return () => {
      mounted = false;
    };
  }, [engine.id, engine.config]);

  const deleteMutation = useMutation({
    mutationFn: (id: string) => deleteEngine({ data: id }),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: ['engines'] });
      toast.success('Engine configuration deleted');
    },
    onError: (error: Error) => {
      toast.error(`Failed to delete engine: ${error.message}`);
    },
  });

  const getStatusColor = () => {
    switch (status) {
      case 'available':
        return 'bg-emerald-500/15 text-emerald-600 dark:text-emerald-400 border-emerald-500/20';
      case 'unavailable':
        return 'bg-rose-500/15 text-rose-600 dark:text-rose-400 border-rose-500/20';
      case 'error':
        return 'bg-amber-500/15 text-amber-600 dark:text-amber-400 border-amber-500/20';
      default:
        return 'bg-muted text-muted-foreground border-border/50';
    }
  };

  return (
    <Card className="relative h-full flex flex-col transition-all duration-500 hover:-translate-y-1 hover:shadow-2xl hover:shadow-primary/10 group overflow-hidden bg-gradient-to-br from-background/80 to-background/40 backdrop-blur-xl border-border/40 hover:border-primary/20">
      {/* Status Line (Gradient like JobCard but colored by status) */}
      <div
        className={cn(
          'absolute inset-x-0 top-0 h-0.5 transition-opacity duration-700',
          status === 'available'
            ? 'bg-gradient-to-r from-transparent via-emerald-500/60 to-transparent'
            : status === 'unavailable'
              ? 'bg-gradient-to-r from-transparent via-rose-500/60 to-transparent'
              : status === 'error'
                ? 'bg-gradient-to-r from-transparent via-amber-500/60 to-transparent'
                : 'bg-gradient-to-r from-transparent via-muted-foreground/20 to-transparent',
          'opacity-50 group-hover:opacity-100',
        )}
      />

      {/* Hover Glow Effect */}
      <div className="absolute -inset-0.5 bg-gradient-to-br from-primary/5 to-transparent opacity-0 group-hover:opacity-100 blur-2xl transition-opacity duration-500 pointer-events-none" />

      <CardHeader className="relative flex flex-row items-center gap-4 pb-2 space-y-0 z-10">
        <div className="p-3 rounded-2xl bg-primary/10 text-primary ring-1 ring-inset ring-black/5 dark:ring-white/5 transition-transform duration-500 group-hover:scale-110 group-hover:rotate-3">
          <Cpu className="h-5 w-5" />
        </div>
        <div className="flex-1 min-w-0 space-y-1">
          <CardTitle className="text-base font-medium truncate tracking-tight text-foreground/90 group-hover:text-primary transition-colors duration-300">
            {engine.name}
          </CardTitle>
          <div className="flex items-center gap-2">
            <span className="text-[10px] uppercase tracking-wider font-semibold text-muted-foreground/60">
              {engine.engine_type}
            </span>
          </div>
        </div>
        <Badge
          variant="outline"
          className={cn(
            'capitalize font-medium border px-2 py-0.5 h-auto text-[10px]',
            getStatusColor(),
          )}
        >
          {status === 'loading' ? (
            <Trans>Checking</Trans>
          ) : status === 'available' ? (
            <Trans>Ready</Trans>
          ) : status === 'unavailable' ? (
            <Trans>Down</Trans>
          ) : (
            <Trans>Error</Trans>
          )}
        </Badge>
      </CardHeader>

      <CardContent className="relative pb-4 flex-1 z-10 space-y-4">
        <div className="flex items-center justify-between p-3 rounded-lg bg-muted/40 border border-border/50 backdrop-blur-sm group-hover:bg-muted/60 transition-colors">
          <span className="text-xs font-medium text-muted-foreground flex items-center gap-1.5">
            {status === 'loading' && (
              <Loader2 className="w-3.5 h-3.5 animate-spin" />
            )}
            {status === 'available' && (
              <CheckCircle2 className="w-3.5 h-3.5 text-emerald-500" />
            )}
            {status === 'unavailable' && (
              <XCircle className="w-3.5 h-3.5 text-rose-500" />
            )}
            {status === 'error' && (
              <AlertCircle className="w-3.5 h-3.5 text-amber-500" />
            )}
            <Trans>System Status</Trans>
          </span>
          <span
            className={cn(
              'text-[10px] font-mono',
              status === 'available'
                ? 'text-emerald-600 dark:text-emerald-400'
                : 'text-muted-foreground',
            )}
          >
            {status === 'loading'
              ? '...'
              : status === 'available'
                ? 'ONLINE'
                : status === 'unavailable'
                  ? 'OFFLINE'
                  : 'ERROR'}
          </span>
        </div>

        {version && (
          <div className="space-y-1.5">
            <span className="text-[10px] uppercase tracking-wider text-muted-foreground font-semibold ml-1">
              <Trans>Version</Trans>
            </span>
            <div className="text-xs bg-background/50 p-2.5 rounded-md font-mono border border-border/50 text-foreground/80 break-all shadow-sm group-hover:border-primary/20 transition-colors">
              {version}
            </div>
          </div>
        )}
      </CardContent>

      <CardFooter className="relative pt-0 text-[10px] text-muted-foreground flex justify-end items-center z-10 border-t border-border/20 mt-auto px-6 py-3 bg-muted/5 gap-2">
        <Button
          asChild
          className="h-9 px-4 text-sm font-medium bg-gradient-to-r from-primary to-primary/80 hover:from-primary/90 hover:to-primary text-primary-foreground shadow-sm hover:shadow-primary/25 hover:scale-[1.02] transition-all duration-300 border-0"
        >
          <Link to="/config/engines/$engineId" params={{ engineId: engine.id }}>
            <Settings2 className="w-4 h-4 mr-2" />
            <Trans>Configure</Trans>
          </Link>
        </Button>

        <AlertDialog>
          <AlertDialogTrigger asChild>
            <Button
              variant="ghost"
              size="icon"
              className="h-9 w-9 text-muted-foreground hover:text-rose-600 hover:bg-rose-500/10 transition-colors"
            >
              <Trash2 className="w-4 h-4" />
            </Button>
          </AlertDialogTrigger>
          <AlertDialogContent>
            <AlertDialogHeader>
              <AlertDialogTitle>
                <Trans>Delete Engine?</Trans>
              </AlertDialogTitle>
              <AlertDialogDescription>
                <Trans>
                  This will permanently remove the "{engine.name}"
                  configuration. This action cannot be undone.
                </Trans>
              </AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
              <AlertDialogCancel>
                <Trans>Cancel</Trans>
              </AlertDialogCancel>
              <AlertDialogAction
                onClick={() => deleteMutation.mutate(engine.id)}
                className="bg-rose-600 hover:bg-rose-700 text-white"
              >
                <Trans>Delete</Trans>
              </AlertDialogAction>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>
      </CardFooter>
    </Card>
  );
}

export function CreateEngineCard() {
  return (
    <Link
      to="/config/engines/create"
      className="group relative flex flex-col h-full min-h-[250px]"
    >
      <Card className="relative h-full flex flex-col items-center justify-center border-dashed border-2 border-muted/60 hover:border-primary/40 bg-muted/5 hover:bg-muted/10 transition-all duration-500 cursor-pointer overflow-hidden hover:shadow-2xl hover:shadow-primary/5">
        {/* Hover Glow */}
        <div className="absolute -inset-0.5 bg-gradient-to-br from-primary/5 to-transparent opacity-0 group-hover:opacity-100 blur-2xl transition-opacity duration-500 pointer-events-none" />

        <div className="relative z-10 flex flex-col items-center justify-center p-6 text-center space-y-4">
          <div className="p-4 rounded-full bg-background/50 ring-1 ring-border/50 group-hover:bg-primary/10 group-hover:ring-primary/20 group-hover:scale-110 transition-all duration-500 shadow-sm">
            <Plus className="w-8 h-8 text-muted-foreground/60 group-hover:text-primary transition-colors" />
          </div>

          <div className="space-y-1.5">
            <h3 className="font-semibold text-lg group-hover:text-primary transition-colors duration-300">
              <Trans>Add New Engine</Trans>
            </h3>
            <p className="text-sm text-muted-foreground/80 max-w-[200px] font-light">
              <Trans>Configure a new download tool.</Trans>
            </p>
          </div>

          <Button
            size="sm"
            variant="secondary"
            className="mt-4 opacity-0 translate-y-2 group-hover:opacity-100 group-hover:translate-y-0 transition-all duration-300 pointer-events-none bg-background/50 backdrop-blur-sm border-primary/10"
          >
            <PlayCircle className="w-4 h-4 mr-2" />
            <Trans>Get Started</Trans>
          </Button>
        </div>
      </Card>
    </Link>
  );
}
