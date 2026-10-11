import { useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { toast } from 'sonner';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import {
  AlertCircle,
  CheckCircle2,
  Cpu,
  Loader2,
  MoreHorizontal,
  Plus,
  Settings2,
  Trash2,
  XCircle,
} from 'lucide-react';
import { z } from 'zod';
import { EngineConfigSchema } from '@/api/schemas';
import { enginesQueryOptions } from '@/api/engines';
import { testEngine, deleteEngine } from '@/server/functions';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
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
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import {
  ListPanel,
  ListPanelEmpty,
  ListPanelRows,
  ROW_BUTTON,
} from '@/components/shared/list-panel';
import { useMenuDialog } from '@/hooks/use-menu-dialog';
import { cn } from '@/lib/utils';

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

/** The tint of an engine's avatar by its type. */
const TYPE_TONE: Record<string, string> = {
  FFMPEG: 'bg-emerald-500/15 text-emerald-700 dark:text-emerald-300',
  MESIO: 'bg-sky-500/15 text-sky-700 dark:text-sky-300',
  STREAMLINK: 'bg-violet-500/15 text-violet-700 dark:text-violet-300',
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

/**
 * One engine: its type, name, installed version and status, with a Configure
 * button and a menu for deleting it.
 */
function EngineRow({ engine }: { engine: Engine }) {
  const { i18n } = useLingui();
  const queryClient = useQueryClient();
  const [confirmDelete, setConfirmDelete] = useState(false);
  const { menuButtonRef, openDialog, onCloseAutoFocus } = useMenuDialog();

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

  const remove = useMutation({
    mutationFn: (id: string) => deleteEngine({ data: id }),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: ['engines'] });
      toast.success(i18n._(msg`Engine deleted`));
    },
    onError: (error: Error) => {
      toast.error(error.message || i18n._(msg`Failed to delete engine`));
    },
  });

  const detail =
    status === 'available' && data?.version
      ? { content: data.version, className: 'font-mono' }
      : status === 'unavailable'
        ? { content: i18n._(msg`This tool was not found on the server.`) }
        : status === 'error'
          ? {
              content: i18n._(
                msg`Could not check whether this tool is installed.`,
              ),
            }
          : undefined;

  return (
    <li aria-busy={remove.isPending || undefined}>
      <div className="flex items-center gap-x-2 px-3 py-2.5 transition-colors hover:bg-muted/30 sm:gap-x-3 sm:px-4">
        <span
          aria-hidden="true"
          className={cn(
            'flex size-9 shrink-0 items-center justify-center rounded-full',
            TYPE_TONE[engine.engine_type] ?? 'bg-primary/10 text-primary',
          )}
        >
          <Cpu className="size-4" />
        </span>
        <div className="min-w-0 flex-1">
          <p className="truncate text-sm font-medium">{engine.name}</p>
          {/* The type stays whole and the detail truncates beside it: a long
              version string such as ffmpeg's would otherwise wrap out of sight. */}
          <p
            className="mt-0.5 flex min-w-0 gap-1 text-xs text-muted-foreground"
            title={
              detail
                ? `${engine.engine_type} · ${detail.content}`
                : engine.engine_type
            }
          >
            <span className="shrink-0">{engine.engine_type}</span>
            {detail && (
              <>
                <span aria-hidden="true">·</span>
                <span className={cn('truncate', detail.className)}>
                  {detail.content}
                </span>
              </>
            )}
          </p>
        </div>
        {remove.isPending ? (
          <span
            role="status"
            className="flex items-center gap-1.5 px-1 text-xs text-muted-foreground"
          >
            <Loader2 className="h-3.5 w-3.5 animate-spin" />
            <Trans>Deleting…</Trans>
          </span>
        ) : (
          <Badge
            variant="outline"
            className={cn(
              'shrink-0 gap-1.5 whitespace-nowrap',
              STATUS_STYLE[status],
            )}
          >
            <StatusIcon status={status} />
            <StatusLabel status={status} />
          </Badge>
        )}
        <Button
          asChild
          size="sm"
          variant="outline"
          className={cn(ROW_BUTTON, 'hidden @md:inline-flex')}
        >
          <Link to="/config/engines/$engineId" params={{ engineId: engine.id }}>
            <Settings2 className="h-3.5 w-3.5" />
            <Trans>Configure</Trans>
          </Link>
        </Button>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button
              ref={menuButtonRef}
              type="button"
              size="icon"
              variant="ghost"
              className="size-8 shrink-0 rounded-lg text-muted-foreground"
              aria-label={i18n._(msg`More actions for ${engine.name}`)}
            >
              <MoreHorizontal className="h-4 w-4" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent
            align="end"
            className="w-48 rounded-xl"
            onCloseAutoFocus={onCloseAutoFocus}
          >
            <DropdownMenuItem asChild>
              <Link
                to="/config/engines/$engineId"
                params={{ engineId: engine.id }}
              >
                <Settings2 /> <Trans>Configure</Trans>
              </Link>
            </DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem
              variant="destructive"
              disabled={remove.isPending}
              onSelect={openDialog(() => setConfirmDelete(true))}
            >
              <Trash2 /> <Trans>Delete</Trans>
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
      <AlertDialog open={confirmDelete} onOpenChange={setConfirmDelete}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              <Trans>Delete this engine?</Trans>
            </AlertDialogTitle>
            <AlertDialogDescription>
              <Trans>
                "{engine.name}" will be removed. Streamers using it fall back to
                the default engine.
              </Trans>
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>
              <Trans>Cancel</Trans>
            </AlertDialogCancel>
            <AlertDialogAction
              onClick={() => remove.mutate(engine.id)}
              className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
            >
              <Trans>Delete</Trans>
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </li>
  );
}

/** The configured download engines, each checked for being installed. */
export function EnginesPanel() {
  const engines = useQuery(enginesQueryOptions);
  const addButton = (variant: 'outline' | 'default') => (
    <Button
      asChild
      variant={variant}
      size="sm"
      className="h-8 shrink-0 gap-1.5 rounded-lg"
    >
      <Link to="/config/engines/create">
        <Plus className="h-4 w-4" />
        <Trans>Add engine</Trans>
      </Link>
    </Button>
  );
  return (
    <ListPanel
      title={<Trans>Engines</Trans>}
      description={
        <Trans>
          The download tools recordings can use. Each one is checked on the
          server to confirm it is installed.
        </Trans>
      }
      // While the list loads or is empty, its own empty state offers the only
      // add button.
      action={
        (engines.error || Boolean(engines.data?.length)) && addButton('outline')
      }
    >
      <ListPanelRows
        query={engines}
        empty={
          <ListPanelEmpty
            icon={Cpu}
            title={<Trans>No engines yet</Trans>}
            help={<Trans>Configure a download tool to record with.</Trans>}
            action={addButton('default')}
          />
        }
        renderRow={(engine) => <EngineRow key={engine.id} engine={engine} />}
      />
    </ListPanel>
  );
}
