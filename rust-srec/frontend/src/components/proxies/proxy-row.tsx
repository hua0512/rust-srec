import { useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { Trans } from '@lingui/react/macro';
import { msg, t } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import {
  Copy,
  Gauge,
  Globe,
  Loader2,
  Lock,
  MoreHorizontal,
  Pencil,
  ShieldCheck,
  Trash2,
  Waypoints,
} from 'lucide-react';
import { Button } from '@/components/ui/button';
import {
  Disclosure,
  DisclosureContent,
  DisclosureTrigger,
} from '@/components/ui/disclosure';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import { DotSeparated } from '@/components/shared/list-panel';
import { useMenuDialog } from '@/hooks/use-menu-dialog';
import { cn } from '@/lib/utils';
import { proxyDetailQueryOptions } from '@/api/proxies';
import type { SavedProxy } from '@/api/schemas/proxies';
import { proxyAddress } from './proxy-route-label';
import { ProxyReferencesList } from './proxy-references';

/** The icon and tint of a proxy by how it connects. */
const SCHEME_STYLES = {
  http: {
    icon: Globe,
    tone: 'bg-sky-500/10 text-sky-600 dark:text-sky-400',
  },
  https: {
    icon: ShieldCheck,
    tone: 'bg-emerald-500/10 text-emerald-600 dark:text-emerald-400',
  },
  socks: {
    icon: Waypoints,
    tone: 'bg-violet-500/10 text-violet-600 dark:text-violet-400',
  },
} as const;

function schemeStyle(scheme: string) {
  if (scheme === 'https') return SCHEME_STYLES.https;
  if (scheme.startsWith('socks')) return SCHEME_STYLES.socks;
  return SCHEME_STYLES.http;
}

interface ProxyRowActions {
  onEdit: () => void;
  onTest: () => void;
  onDelete: () => void;
  onCopy: () => void;
  /** Set while the proxy is being deleted. */
  deleting?: boolean;
}

/** Where a proxy is used, loaded when its row is first expanded. */
function ProxyUsage({ proxy }: { proxy: SavedProxy }) {
  const detail = useQuery(proxyDetailQueryOptions(proxy.id));
  if (detail.isPending)
    return (
      <p className="flex items-center gap-1.5 text-muted-foreground">
        <Loader2 className="size-3 animate-spin" />
        <Trans>Loading…</Trans>
      </p>
    );
  if (detail.error)
    return <p className="text-destructive">{detail.error.message}</p>;
  return <ProxyReferencesList references={detail.data.references} />;
}

/**
 * One saved proxy row: its protocol, name, address, login and usage pill that
 * expands to show full details and references, plus a direct Test button and
 * menu for secondary actions.
 */
export function ProxyRow({
  proxy,
  actions,
}: {
  proxy: SavedProxy;
  actions: ProxyRowActions;
}) {
  const { i18n } = useLingui();
  const [open, setOpen] = useState(false);
  // Where it is used loads on the first expansion and stays for the closing
  // animation.
  const [expanded, setExpanded] = useState(false);
  const usage = proxy.usage_count;
  const { menuButtonRef, openDialog, onCloseAutoFocus } = useMenuDialog();
  const { icon: Icon, tone } = schemeStyle(proxy.scheme);

  const subParts = [
    { content: proxyAddress(proxy), className: 'font-mono' },
    ...(proxy.username ? [{ content: proxy.username }] : []),
  ];

  return (
    <li aria-busy={actions.deleting || undefined} data-proxy-id={proxy.id}>
      <Disclosure
        open={open}
        onOpenChange={(next) => {
          setOpen(next);
          if (next) setExpanded(true);
        }}
      >
        <div className="flex flex-wrap items-center gap-3 px-4 py-3.5 transition-colors hover:bg-muted/35 sm:flex-nowrap">
          <span
            aria-hidden="true"
            className={cn(
              'flex size-9 shrink-0 items-center justify-center rounded-xl ring-1 ring-inset ring-black/5 dark:ring-white/5',
              tone,
            )}
          >
            <Icon className="size-4" />
          </span>

          <div className="min-w-0 flex-1">
            <div className="flex flex-wrap items-center gap-2">
              <span className="truncate text-sm font-semibold text-foreground">
                {proxy.name}
              </span>
              <span className="rounded-md bg-muted px-1.5 py-0.5 text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
                {proxy.scheme}
              </span>
              {proxy.username && (
                <span className="inline-flex items-center gap-1 rounded-md bg-amber-500/10 px-1.5 py-0.5 text-[10px] font-semibold text-amber-700 dark:text-amber-400">
                  <Lock className="size-2.5" aria-hidden="true" />
                  <Trans context="proxy auth badge">Auth</Trans>
                </span>
              )}
            </div>
            <p
              className="mt-0.5 text-xs text-muted-foreground"
              title={subParts.map((part) => part.content).join(' · ')}
            >
              <DotSeparated parts={subParts} />
            </p>
          </div>

          <div className="ml-auto flex shrink-0 items-center gap-2">
            <DisclosureTrigger
              aria-label={i18n._(msg`Details for ${proxy.name}`)}
              className={cn(
                'w-auto gap-1.5 rounded-full px-2.5 py-1 text-xs font-medium whitespace-nowrap transition-colors [&_svg]:size-3.5',
                usage > 0
                  ? 'bg-muted text-foreground hover:bg-muted/80'
                  : 'text-muted-foreground hover:bg-muted/60 hover:text-foreground',
              )}
            >
              {usage > 0 ? t(i18n)`used by ${usage}` : t(i18n)`not used`}
            </DisclosureTrigger>

            {actions.deleting ? (
              <span
                role="status"
                className="flex items-center gap-1.5 px-1 text-xs text-muted-foreground"
              >
                <Loader2 className="h-3.5 w-3.5 animate-spin" />
                <Trans>Deleting…</Trans>
              </span>
            ) : (
              <Button
                type="button"
                size="sm"
                variant="outline"
                className="h-8 gap-1.5 rounded-lg px-2.5 text-xs"
                onClick={actions.onTest}
              >
                <Gauge className="h-3.5 w-3.5" />
                <Trans>Test</Trans>
              </Button>
            )}

            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button
                  ref={menuButtonRef}
                  type="button"
                  size="icon"
                  variant="ghost"
                  className="size-8 shrink-0 rounded-lg text-muted-foreground"
                  aria-label={i18n._(msg`More actions for ${proxy.name}`)}
                >
                  <MoreHorizontal className="h-4 w-4" />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent
                align="end"
                className="w-48 rounded-xl"
                onCloseAutoFocus={onCloseAutoFocus}
              >
                <DropdownMenuItem onSelect={openDialog(actions.onEdit)}>
                  <Pencil /> <Trans>Edit</Trans>
                </DropdownMenuItem>
                <DropdownMenuItem onSelect={actions.onCopy}>
                  <Copy /> <Trans>Copy address</Trans>
                </DropdownMenuItem>
                <DropdownMenuSeparator />
                <DropdownMenuItem
                  variant="destructive"
                  disabled={actions.deleting}
                  onSelect={openDialog(actions.onDelete)}
                >
                  <Trash2 /> <Trans>Delete</Trans>
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          </div>
        </div>

        <DisclosureContent className="space-y-2.5 border-t border-dashed border-border/60 bg-muted/20 px-4 py-3 text-xs sm:pl-16">
          <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-1.5">
            <dt className="text-muted-foreground">
              <Trans>Address</Trans>
            </dt>
            <dd className="font-mono break-all">{proxy.url}</dd>
            <dt className="text-muted-foreground">
              <Trans>Login</Trans>
            </dt>
            <dd>
              {proxy.username ? (
                <>
                  {proxy.username}
                  {proxy.has_password && (
                    <span className="text-muted-foreground/80">
                      {' '}
                      · <Trans>password saved</Trans>
                    </span>
                  )}
                </>
              ) : (
                <span className="text-muted-foreground/80">
                  <Trans context="no proxy login">None</Trans>
                </span>
              )}
            </dd>
          </dl>
          {expanded && <ProxyUsage proxy={proxy} />}
        </DisclosureContent>
      </Disclosure>
    </li>
  );
}
