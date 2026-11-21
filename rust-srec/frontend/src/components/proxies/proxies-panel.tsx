import { useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { motion } from 'motion/react';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { Lock, Monitor, Network, Plus } from 'lucide-react';
import { toast } from 'sonner';
import { useCopyToClipboard } from '@/hooks/use-copy-to-clipboard';
import { Button } from '@/components/ui/button';
import { Skeleton } from '@/components/ui/skeleton';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { SettingsCard } from '@/components/config/settings-card';
import { ListPanelEmpty } from '@/components/shared/list-panel';
import { QueryError } from '@/components/shared/query-error';
import { itemVariants } from '@/lib/animation';
import { cn } from '@/lib/utils';
import { deleteProxy } from '@/server/functions/proxies';
import {
  invalidateProxyQueries,
  proxiesQueryOptions,
  proxyDetailQueryOptions,
  systemProxyQueryOptions,
} from '@/api/proxies';
import type { ProxyReferences, SavedProxy } from '@/api/schemas/proxies';
import { DeleteProxyDialog } from './delete-proxy-dialog';
import { ProxyEditorDialog } from './proxy-editor-dialog';
import { proxyReferencesFromConflict } from './proxy-errors';
import { proxyAddress } from './proxy-route-label';
import { ProxyRow } from './proxy-row';
import { ProxyTestPanel } from './proxy-test';

/** The proxy read from the server's environment at startup. */
function SystemProxyCard() {
  const system = useQuery(systemProxyQueryOptions).data;
  if (!system) return null;

  return (
    <motion.div variants={itemVariants}>
      <SettingsCard
        title={<Trans>System proxy</Trans>}
        description={
          <Trans>
            Read from the server&apos;s environment at startup. Used whenever a
            route is set to System proxy.
          </Trans>
        }
        icon={Monitor}
        iconColor={
          system.detected
            ? 'text-emerald-600 dark:text-emerald-400'
            : 'text-muted-foreground'
        }
        iconBgColor={system.detected ? 'bg-emerald-500/10' : 'bg-muted/60'}
        action={
          <div className="flex items-center gap-2">
            {system.detected && system.authenticated && (
              <span className="inline-flex items-center gap-1 rounded-full bg-amber-500/10 px-2.5 py-1 text-xs font-medium text-amber-700 dark:text-amber-400">
                <Lock className="size-3" aria-hidden="true" />
                <Trans context="proxy auth badge">Auth</Trans>
              </span>
            )}
            <span
              className={cn(
                'inline-flex items-center gap-1.5 rounded-full px-2.5 py-1 text-xs font-medium whitespace-nowrap',
                system.detected
                  ? 'bg-emerald-500/10 text-emerald-700 dark:text-emerald-400'
                  : 'bg-muted/60 text-muted-foreground',
              )}
            >
              <span
                className={cn(
                  'size-1.5 rounded-full',
                  system.detected ? 'bg-emerald-500' : 'bg-muted-foreground',
                )}
                aria-hidden="true"
              />
              {system.detected ? (
                <Trans>Detected</Trans>
              ) : (
                <Trans>Not detected</Trans>
              )}
            </span>
          </div>
        }
      >
        {system.detected ? (
          <div className="flex flex-wrap items-center justify-between gap-2 rounded-xl border border-border/50 bg-muted/30 px-3.5 py-2.5 text-xs">
            <code className="font-mono font-medium break-all text-foreground">
              {system.url ?? ''}
            </code>
            {system.no_proxy && (
              <span className="font-mono text-muted-foreground break-all">
                NO_PROXY: {system.no_proxy}
              </span>
            )}
          </div>
        ) : (
          <p className="text-sm text-muted-foreground">
            <Trans>No system proxy detected</Trans>
          </p>
        )}
      </SettingsCard>
    </motion.div>
  );
}

/** Checks a saved proxy against a site of the user's choice. */
function ProxyTestDialog({
  proxy,
  onClose,
}: {
  proxy: SavedProxy;
  onClose: () => void;
}) {
  const address = proxyAddress(proxy);
  const name = proxy.name;
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
    >
      <DialogContent className="border-border/50 sm:max-w-md">
        <DialogHeader>
          <DialogTitle>
            <Trans>Test “{name}”</Trans>
          </DialogTitle>
          <DialogDescription>
            <Trans>
              Requests a page once through{' '}
              <span className="font-mono">{address}</span> and reports whether
              it answered.
            </Trans>
          </DialogDescription>
        </DialogHeader>
        <ProxyTestPanel autoFocus endpoint={() => ({ proxy_id: proxy.id })} />
      </DialogContent>
    </Dialog>
  );
}

function ProxiesListSkeleton() {
  return (
    <div className="divide-y divide-border/50 overflow-hidden rounded-xl border border-border/50">
      {[0, 1, 2].map((index) => (
        <div key={index} className="flex items-center gap-3 px-4 py-3.5">
          <Skeleton className="size-9 shrink-0 rounded-xl" />
          <div className="min-w-0 flex-1 space-y-2">
            <Skeleton className="h-4 w-36 max-w-full" />
            <Skeleton className="h-3 w-56 max-w-full" />
          </div>
          <Skeleton className="h-6 w-20 rounded-full" />
          <Skeleton className="h-8 w-16 rounded-lg" />
        </div>
      ))}
    </div>
  );
}

/**
 * The saved proxies: add, edit, test, copy and delete. Every change is saved
 * right away.
 */
export function ProxiesPanel() {
  const { i18n } = useLingui();
  const queryClient = useQueryClient();
  const proxies = useQuery(proxiesQueryOptions);
  const [editor, setEditor] = useState<{ proxy?: SavedProxy }>();
  const [testing, setTesting] = useState<SavedProxy>();
  const [deleting, setDeleting] = useState<{
    proxy: SavedProxy;
    references?: ProxyReferences;
  }>();
  const [pendingDelete, setPendingDelete] = useState<string>();

  const remove = useMutation({
    mutationFn: (proxy: SavedProxy) =>
      deleteProxy({
        data: { id: proxy.id, expected_version: proxy.version },
      }),
    onMutate: (proxy) => setPendingDelete(proxy.id),
    onSettled: () => setPendingDelete(undefined),
    onSuccess: (_data, proxy) => {
      const name = proxy.name;
      toast.success(i18n._(msg`Deleted “${name}”`));
      void invalidateProxyQueries(queryClient);
    },
    onError: (error: Error, proxy) => {
      // The usage count was stale: show what the backend says still uses it.
      const references = proxyReferencesFromConflict(error);
      if (references) setDeleting({ proxy, references });
      else toast.error(error.message);
      void invalidateProxyQueries(queryClient);
    },
  });

  /** Asks to confirm, or lists what uses the proxy when something does. */
  const askDelete = async (proxy: SavedProxy) => {
    if (proxy.usage_count === 0) {
      setDeleting({ proxy });
      return;
    }
    try {
      const detail = await queryClient.fetchQuery({
        ...proxyDetailQueryOptions(proxy.id),
        staleTime: 0,
      });
      setDeleting({ proxy: detail.proxy, references: detail.references });
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    }
  };

  const { copy: copyText } = useCopyToClipboard();
  const copy = (proxy: SavedProxy) =>
    copyText(proxy.url, i18n._(msg`Address copied`));

  const addButton = (
    <Button
      type="button"
      className="shrink-0 gap-2"
      onClick={() => setEditor({})}
    >
      <Plus className="h-4 w-4" />
      <Trans>Add proxy</Trans>
    </Button>
  );

  return (
    <>
      <motion.div variants={itemVariants}>
        <SettingsCard
          title={<Trans>Proxies</Trans>}
          description={
            <Trans>
              Saved proxies that global, platform, template, streamer, account,
              notification channel and upload step settings can route requests
              through. Changes are saved right away.
            </Trans>
          }
          icon={Network}
          action={(proxies.error || Boolean(proxies.data?.length)) && addButton}
        >
          {proxies.error && !proxies.data ? (
            <QueryError
              title={<Trans>Failed to load proxies</Trans>}
              error={proxies.error}
              onRetry={() => void proxies.refetch()}
              className="mt-2"
            />
          ) : proxies.isLoading ? (
            <ProxiesListSkeleton />
          ) : !proxies.data || proxies.data.length === 0 ? (
            <ListPanelEmpty
              icon={Network}
              title={<Trans>No proxies yet</Trans>}
              help={
                <Trans>
                  Add one to send recordings, checks and danmu through it. Until
                  then, requests connect directly or through the system proxy,
                  as the global settings choose.
                </Trans>
              }
              action={addButton}
            />
          ) : (
            <ul className="divide-y divide-border/50 overflow-hidden rounded-xl border border-border/50">
              {proxies.data.map((proxy) => (
                <ProxyRow
                  key={proxy.id}
                  proxy={proxy}
                  actions={{
                    onEdit: () => setEditor({ proxy }),
                    onTest: () => setTesting(proxy),
                    onDelete: () => void askDelete(proxy),
                    onCopy: () => void copy(proxy),
                    deleting: pendingDelete === proxy.id,
                  }}
                />
              ))}
            </ul>
          )}
        </SettingsCard>
      </motion.div>

      <SystemProxyCard />

      {editor && (
        <ProxyEditorDialog
          key={editor.proxy?.id ?? 'new'}
          proxy={editor.proxy}
          onClose={() => setEditor(undefined)}
          onSaved={() => setEditor(undefined)}
        />
      )}
      {testing && (
        <ProxyTestDialog
          proxy={testing}
          onClose={() => setTesting(undefined)}
        />
      )}
      <DeleteProxyDialog
        target={deleting}
        onClose={() => setDeleting(undefined)}
        onConfirm={(proxy) => remove.mutate(proxy)}
      />
    </>
  );
}
