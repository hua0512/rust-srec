import {
  Card,
  CardContent,
  CardFooter,
  CardHeader,
} from '@/components/ui/card';
import { Button } from '@/components/ui/button';
import { Badge } from '@/components/ui/badge';
import {
  AlertCircle,
  CheckCircle2,
  Cpu,
  Loader2,
  Plus,
  Settings2,
  Trash2,
  XCircle,
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
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { toast } from 'sonner';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { cn } from '@/lib/utils';
import { Link } from '@tanstack/react-router';

type Engine = z.infer<typeof EngineConfigSchema>;

type EngineStatus = 'loading' | 'available' | 'unavailable' | 'error';

const STATUS_STYLE: Record<EngineStatus, string> = {
  loading: 'bg-muted text-muted-foreground border-border/50',
  available:
    'bg-emerald-500/10 text-emerald-700 dark:text-emerald-400 border-emerald-500/20',
  unavailable:
    'bg-rose-500/10 text-rose-700 dark:text-rose-400 border-rose-500/20',
  error:
    'bg-amber-500/10 text-amber-700 dark:text-amber-400 border-amber-500/20',
};

function StatusIcon({ status }: { status: EngineStatus }) {
  const className = 'h-3.5 w-3.5 shrink-0';
  switch (status) {
    case 'loading':
      return <Loader2 className={cn(className, 'animate-spin')} />;
    case 'available':
      return <CheckCircle2 className={className} />;
    case 'unavailable':
      return <XCircle className={className} />;
    case 'error':
      return <AlertCircle className={className} />;
  }
}

function StatusLabel({ status }: { status: EngineStatus }) {
  switch (status) {
    case 'loading':
      return <Trans>Checking</Trans>;
    case 'available':
      return <Trans>Ready</Trans>;
    case 'unavailable':
      return <Trans>Not installed</Trans>;
    case 'error':
      return <Trans>Check failed</Trans>;
  }
}

export function EngineCard({ engine }: { engine: Engine }) {
  const { i18n } = useLingui();
  const queryClient = useQueryClient();

  // Keyed on the config as well as the id, so editing an engine re-checks it. Going through
  // react-query rather than a bare effect also dedupes the probe and keeps the result cached
  // while navigating between config pages.
  const { data, isPending, isError } = useQuery({
    queryKey: ['engine-test', engine.id, engine.config],
    queryFn: () => testEngine({ data: engine.id }),
    retry: false,
    staleTime: 60_000,
  });

  const status: EngineStatus = isPending
    ? 'loading'
    : isError
      ? 'error'
      : data?.available
        ? 'available'
        : 'unavailable';

  const deleteMutation = useMutation({
    mutationFn: (id: string) => deleteEngine({ data: id }),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: ['engines'] });
      toast.success(i18n._(msg`Engine deleted`));
    },
    onError: (error: Error) => {
      toast.error(error.message || i18n._(msg`Failed to delete engine`));
    },
  });

  return (
    <Card className="flex h-full flex-col border-border/50 shadow-sm transition-shadow hover:shadow-md">
      <CardHeader className="flex flex-row items-start gap-3 space-y-0 pb-3">
        <span className="flex h-10 w-10 shrink-0 items-center justify-center rounded-lg bg-primary/10 text-primary">
          <Cpu className="h-5 w-5" />
        </span>
        <div className="min-w-0 flex-1 space-y-0.5">
          <h3 className="truncate font-semibold leading-tight">
            {engine.name}
          </h3>
          <p className="text-xs uppercase tracking-wider text-muted-foreground">
            {engine.engine_type}
          </p>
        </div>
        {/* The single place status is stated; the body carries the version detail only. */}
        <Badge
          variant="outline"
          className={cn('gap-1.5 whitespace-nowrap', STATUS_STYLE[status])}
        >
          <StatusIcon status={status} />
          <StatusLabel status={status} />
        </Badge>
      </CardHeader>

      <CardContent className="flex-1">
        {status === 'available' && data?.version ? (
          <div className="space-y-1.5">
            <p className="text-[11px] font-bold uppercase tracking-wider text-muted-foreground">
              <Trans>Version</Trans>
            </p>
            <p className="break-all rounded-md border bg-muted/40 px-2.5 py-2 font-mono text-xs">
              {data.version}
            </p>
          </div>
        ) : status === 'unavailable' ? (
          <p className="text-sm text-muted-foreground">
            <Trans>This tool was not found on the server.</Trans>
          </p>
        ) : status === 'error' ? (
          <p className="text-sm text-muted-foreground">
            <Trans>Could not check whether this tool is installed.</Trans>
          </p>
        ) : null}
      </CardContent>

      <CardFooter className="mt-auto justify-end gap-2 border-t pt-4">
        <AlertDialog>
          <AlertDialogTrigger asChild>
            <Button
              type="button"
              variant="ghost"
              size="icon"
              className="text-muted-foreground hover:bg-destructive/10 hover:text-destructive"
            >
              <Trash2 className="h-4 w-4" />
              <span className="sr-only">
                <Trans>Delete engine</Trans>
              </span>
            </Button>
          </AlertDialogTrigger>
          <AlertDialogContent>
            <AlertDialogHeader>
              <AlertDialogTitle>
                <Trans>Delete this engine?</Trans>
              </AlertDialogTitle>
              <AlertDialogDescription>
                <Trans>
                  "{engine.name}" will be removed. Streamers using it fall back
                  to the default engine.
                </Trans>
              </AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
              <AlertDialogCancel>
                <Trans>Cancel</Trans>
              </AlertDialogCancel>
              <AlertDialogAction
                onClick={() => deleteMutation.mutate(engine.id)}
                className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
              >
                <Trans>Delete</Trans>
              </AlertDialogAction>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>

        <Button asChild>
          <Link to="/config/engines/$engineId" params={{ engineId: engine.id }}>
            <Settings2 className="mr-2 h-4 w-4" />
            <Trans>Configure</Trans>
          </Link>
        </Button>
      </CardFooter>
    </Card>
  );
}

export function CreateEngineCard() {
  return (
    <Link
      to="/config/engines/create"
      className="group flex h-full rounded-xl focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2"
    >
      <Card className="flex h-full w-full flex-col items-center justify-center gap-3 border-2 border-dashed border-border/60 p-6 text-center shadow-none transition-colors group-hover:border-primary/40 group-hover:bg-muted/40">
        <span className="flex h-12 w-12 items-center justify-center rounded-full bg-muted transition-colors group-hover:bg-primary/10">
          <Plus className="h-6 w-6 text-muted-foreground transition-colors group-hover:text-primary" />
        </span>
        <div className="space-y-1">
          <p className="font-semibold">
            <Trans>Add an engine</Trans>
          </p>
          <p className="max-w-[220px] text-sm text-muted-foreground">
            <Trans>Configure another download tool.</Trans>
          </p>
        </div>
      </Card>
    </Link>
  );
}
