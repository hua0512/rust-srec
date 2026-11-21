import { useState, type ElementType } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { toast } from 'sonner';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import {
  Cpu,
  Database,
  Loader2,
  Plus,
  Radio,
  Settings2,
  Terminal,
  Trash2,
} from 'lucide-react';
import { z } from 'zod';
import { queryKeys } from '@/api/query-keys';
import { EngineConfigSchema } from '@/api/schemas';
import { enginesQueryOptions } from '@/api/engines';
import { testEngine, deleteEngine } from '@/server/functions';
import { Button } from '@/components/ui/button';
import { Skeleton } from '@/components/ui/skeleton';
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
import { SettingsCard } from '@/components/config/settings-card';
import { DotSeparated, ListPanelEmpty } from '@/components/shared/list-panel';
import { QueryError } from '@/components/shared/query-error';
import { formatBytes } from '@/lib/format';
import { cn } from '@/lib/utils';

type Engine = z.infer<typeof EngineConfigSchema>;

type EngineStatus = 'loading' | 'available' | 'unavailable' | 'error';

const STATUS_STYLE: Record<EngineStatus, { pill: string; dot: string }> = {
  loading: {
    pill: 'bg-muted/60 text-muted-foreground',
    dot: 'bg-muted-foreground',
  },
  available: {
    pill: 'bg-emerald-500/10 text-emerald-700 dark:text-emerald-400',
    dot: 'bg-emerald-500',
  },
  unavailable: {
    pill: 'bg-rose-500/10 text-rose-700 dark:text-rose-400',
    dot: 'bg-rose-500',
  },
  error: {
    pill: 'bg-amber-500/10 text-amber-700 dark:text-amber-400',
    dot: 'bg-amber-500',
  },
};

const ENGINE_META: Record<
  Engine['engine_type'],
  { label: string; icon: ElementType; tone: string }
> = {
  FFMPEG: {
    label: 'FFmpeg',
    icon: Terminal,
    tone: 'bg-emerald-500/10 text-emerald-600 dark:text-emerald-400',
  },
  MESIO: {
    label: 'Mesio',
    icon: Database,
    tone: 'bg-sky-500/10 text-sky-600 dark:text-sky-400',
  },
  STREAMLINK: {
    label: 'Streamlink',
    icon: Radio,
    tone: 'bg-violet-500/10 text-violet-600 dark:text-violet-400',
  },
};

function StatusBadge({ status }: { status: EngineStatus }) {
  const style = STATUS_STYLE[status];
  return (
    <span
      className={cn(
        'inline-flex shrink-0 items-center gap-1.5 rounded-full px-2.5 py-1 text-xs font-medium whitespace-nowrap',
        style.pill,
      )}
    >
      {status === 'loading' ? (
        <Loader2 className="h-3 w-3 animate-spin" aria-hidden="true" />
      ) : (
        <span
          className={cn('size-1.5 rounded-full', style.dot)}
          aria-hidden="true"
        />
      )}
      {status === 'loading' && <Trans>Checking</Trans>}
      {status === 'available' && <Trans>Ready</Trans>}
      {status === 'unavailable' && <Trans>Not installed</Trans>}
      {status === 'error' && <Trans>Check failed</Trans>}
    </span>
  );
}

/** Concise inline config hints shown beside the detected engine version. */
function engineConfigParts(engine: Engine) {
  const parts: { content: string; className?: string }[] = [];
  switch (engine.engine_type) {
    case 'FFMPEG':
      if (engine.config.binary_path) {
        parts.push({
          content: engine.config.binary_path,
          className: 'font-mono',
        });
      }
      if (engine.config.timeout_secs != null) {
        parts.push({ content: `${engine.config.timeout_secs}s` });
      }
      break;
    case 'STREAMLINK':
      if (engine.config.quality) {
        parts.push({ content: engine.config.quality, className: 'font-mono' });
      }
      if (engine.config.binary_path) {
        parts.push({
          content: engine.config.binary_path,
          className: 'font-mono',
        });
      }
      break;
    case 'MESIO': {
      const fixers: string[] = [];
      if (engine.config.fix_flv !== false) fixers.push('FLV');
      if (engine.config.fix_hls !== false) fixers.push('HLS');
      if (engine.config.fix_dash !== false) fixers.push('DASH');
      if (fixers.length > 0) {
        parts.push({ content: fixers.join(' / '), className: 'font-mono' });
      }
      if (engine.config.buffer_size != null) {
        parts.push({ content: formatBytes(engine.config.buffer_size) });
      }
      break;
    }
  }
  return parts;
}

/**
 * One engine row: its type, name, installed version, key settings and live
 * availability, with direct Configure and Delete actions.
 */
function EngineRow({ engine }: { engine: Engine }) {
  const { i18n } = useLingui();
  const queryClient = useQueryClient();
  const [confirmDelete, setConfirmDelete] = useState(false);

  // Keyed on the config as well as the id, so editing an engine re-checks it. Going through
  // react-query rather than a bare effect also dedupes the probe and keeps the result cached
  // while navigating between config pages.
  const { data, isPending, isError } = useQuery({
    queryKey: queryKeys.engines.test(engine.id, engine.config),
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
      await queryClient.invalidateQueries({ queryKey: queryKeys.engines.all });
      toast.success(i18n._(msg`Engine deleted`));
    },
    onError: (error: Error) => {
      toast.error(error.message || i18n._(msg`Failed to delete engine`));
    },
  });

  const meta = ENGINE_META[engine.engine_type];
  const Icon = meta.icon;

  const statusDetail =
    status === 'available' && data?.version
      ? { content: data.version, className: 'font-mono' }
      : status === 'unavailable'
        ? {
            content: i18n._(msg`This tool was not found on the server.`),
            className: 'text-rose-600 dark:text-rose-400',
          }
        : status === 'error'
          ? {
              content: i18n._(
                msg`Could not check whether this tool is installed.`,
              ),
              className: 'text-amber-600 dark:text-amber-400',
            }
          : undefined;

  const parts = [
    ...(statusDetail ? [statusDetail] : []),
    ...engineConfigParts(engine),
  ];

  return (
    <li aria-busy={remove.isPending || undefined}>
      <div className="flex flex-wrap items-center gap-3 px-4 py-3.5 transition-colors hover:bg-muted/35 sm:flex-nowrap">
        <span
          aria-hidden="true"
          className={cn(
            'flex size-9 shrink-0 items-center justify-center rounded-xl ring-1 ring-inset ring-black/5 dark:ring-white/5',
            meta.tone,
          )}
        >
          <Icon className="size-4" />
        </span>

        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2">
            <span className="truncate text-sm font-semibold text-foreground">
              {engine.name}
            </span>
            <span className="rounded-md bg-muted px-1.5 py-0.5 text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
              {meta.label}
            </span>
          </div>
          {parts.length > 0 && (
            <p
              className="mt-0.5 text-xs text-muted-foreground"
              title={parts.map((part) => part.content).join(' · ')}
            >
              <DotSeparated parts={parts} />
            </p>
          )}
        </div>

        <div className="ml-auto flex shrink-0 items-center gap-2">
          {remove.isPending ? (
            <span
              role="status"
              className="flex items-center gap-1.5 px-1 text-xs text-muted-foreground"
            >
              <Loader2 className="h-3.5 w-3.5 animate-spin" />
              <Trans>Deleting…</Trans>
            </span>
          ) : (
            <StatusBadge status={status} />
          )}

          <Button
            asChild
            size="sm"
            variant="outline"
            className="h-8 gap-1.5 rounded-lg px-3 text-xs"
          >
            <Link
              to="/config/engines/$engineId"
              params={{ engineId: engine.id }}
            >
              <Settings2 className="h-3.5 w-3.5" />
              <Trans>Configure</Trans>
            </Link>
          </Button>

          <Button
            type="button"
            size="icon"
            variant="ghost"
            className="size-8 rounded-lg text-muted-foreground hover:bg-destructive/10 hover:text-destructive"
            disabled={remove.isPending}
            aria-label={i18n._(msg`Delete ${engine.name}`)}
            onClick={() => setConfirmDelete(true)}
          >
            <Trash2 className="h-4 w-4" />
          </Button>
        </div>
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

function EnginesListSkeleton() {
  return (
    <div className="divide-y divide-border/50 overflow-hidden rounded-xl border border-border/50">
      {[0, 1, 2].map((index) => (
        <div key={index} className="flex items-center gap-3 px-4 py-3.5">
          <Skeleton className="size-9 shrink-0 rounded-xl" />
          <div className="min-w-0 flex-1 space-y-2">
            <Skeleton className="h-4 w-36 max-w-full" />
            <Skeleton className="h-3 w-64 max-w-full" />
          </div>
          <Skeleton className="h-6 w-16 rounded-full" />
          <Skeleton className="h-8 w-20 rounded-lg" />
        </div>
      ))}
    </div>
  );
}

/** The configured download engines, each checked for being installed. */
export function EnginesPanel() {
  const {
    data: engines,
    isLoading,
    error,
    refetch,
  } = useQuery(enginesQueryOptions);

  const addButton = (
    <Button asChild className="shrink-0 gap-2">
      <Link to="/config/engines/create">
        <Plus className="h-4 w-4" />
        <Trans>Add engine</Trans>
      </Link>
    </Button>
  );

  return (
    <SettingsCard
      title={<Trans>Engines</Trans>}
      description={
        <Trans>
          The download tools recordings can use. Each one is checked on the
          server to confirm it is installed.
        </Trans>
      }
      icon={Cpu}
      action={(error || Boolean(engines?.length)) && addButton}
    >
      {error && !engines ? (
        <QueryError
          title={<Trans>Failed to load engines</Trans>}
          error={error}
          onRetry={() => void refetch()}
          className="mt-2"
        />
      ) : isLoading ? (
        <EnginesListSkeleton />
      ) : !engines || engines.length === 0 ? (
        <ListPanelEmpty
          icon={Cpu}
          title={<Trans>No engines yet</Trans>}
          help={<Trans>Configure a download tool to record with.</Trans>}
          action={addButton}
        />
      ) : (
        <ul className="divide-y divide-border/50 overflow-hidden rounded-xl border border-border/50">
          {engines.map((engine) => (
            <EngineRow key={engine.id} engine={engine} />
          ))}
        </ul>
      )}
    </SettingsCard>
  );
}
