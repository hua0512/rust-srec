import { useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { Monitor, Network, Plus } from 'lucide-react';
import { toast } from 'sonner';
import { Button } from '@/components/ui/button';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import {
  ListPanel,
  ListPanelEmpty,
  ListPanelRows,
} from '@/components/shared/list-panel';
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

/** The proxy the server's environment sets, as the header's last line. */
export function SystemProxyLine() {
  const system = useQuery(systemProxyQueryOptions).data;
  if (!system) return null;
  const url = system.url ?? '';
  return (
    <p className="mt-1 flex items-center gap-1.5">
      <Monitor className="size-3.5 shrink-0" aria-hidden="true" />
      {system.detected ? (
        <span>
          <Trans>
            System proxy: <span className="font-mono">{url}</span>
          </Trans>
        </span>
      ) : (
        <span>
          <Trans>No system proxy detected</Trans>
        </span>
      )}
    </p>
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
  const copy = (proxy: SavedProxy) =>
    navigator.clipboard.writeText(proxy.url).then(
      () => toast.success(i18n._(msg`Address copied`)),
      () => toast.error(i18n._(msg`Could not copy to the clipboard`)),
    );
  const addButton = (variant: 'outline' | 'default') => (
    <Button
      type="button"
      variant={variant}
      size="sm"
      className="h-8 shrink-0 gap-1.5 rounded-lg"
      onClick={() => setEditor({})}
    >
      <Plus className="h-4 w-4" />
      <Trans>Add proxy</Trans>
    </Button>
  );
  return (
    <>
      <ListPanel
        title={<Trans>Proxies</Trans>}
        description={
          <>
            <p>
              <Trans>
                Saved proxies that global, platform, template, streamer and
                account settings can route requests through. Changes are saved
                right away.
              </Trans>
            </p>
            <SystemProxyLine />
          </>
        }
        // While the list loads or is empty, its own empty state offers the
        // only add button.
        action={
          (proxies.error || Boolean(proxies.data?.length)) &&
          addButton('outline')
        }
      >
        <ListPanelRows
          query={proxies}
          empty={
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
              action={addButton('default')}
            />
          }
          renderRow={(proxy) => (
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
          )}
        />
      </ListPanel>
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
