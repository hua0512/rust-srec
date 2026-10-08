import { useId, useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { Trans } from '@lingui/react/macro';
import { msg, t } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import {
  Copy,
  Gauge,
  Globe,
  Loader2,
  MoreHorizontal,
  Pencil,
  ShieldCheck,
  Trash2,
  Waypoints,
} from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Disclosure, DisclosureContent } from '@/components/ui/disclosure';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import { useMenuDialog } from '@/hooks/use-menu-dialog';
import { cn } from '@/lib/utils';
import { proxyDetailQueryOptions } from '@/api/proxies';
import type { SavedProxy } from '@/api/schemas/proxies';
import {
  DotSeparated,
  ROW_BUTTON,
  RowExpandButton,
} from '@/components/shared/list-panel';
import { proxyAddress } from './proxy-route-label';
import { ProxyReferencesList } from './proxy-references';

/** The icon and tint of a proxy by how it connects. */
const SCHEME_STYLES = {
  http: {
    icon: Globe,
    tone: 'bg-sky-500/15 text-sky-700 dark:text-sky-300',
  },
  https: {
    icon: ShieldCheck,
    tone: 'bg-emerald-500/15 text-emerald-700 dark:text-emerald-300',
  },
  socks: {
    icon: Waypoints,
    tone: 'bg-violet-500/15 text-violet-700 dark:text-violet-300',
  },
} as const;

function schemeStyle(scheme: string) {
  if (scheme === 'https') return SCHEME_STYLES.https;
  if (scheme.startsWith('socks')) return SCHEME_STYLES.socks;
  return SCHEME_STYLES.http;
}

/** A proxy's scheme as an icon on its tint. Decorative: the row names it. */
export function ProxyAvatar({ scheme }: { scheme: string }) {
  const { icon: Icon, tone } = schemeStyle(scheme);
  return (
    <span
      aria-hidden="true"
      className={cn(
        'flex size-9 shrink-0 items-center justify-center rounded-full',
        tone,
      )}
    >
      <Icon className="size-4" />
    </span>
  );
}

export interface ProxyRowActions {
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
 * One saved proxy: its scheme, name, address, login and usage, with where it
 * is used once expanded, a Test button and its menu.
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
  const detailsId = useId();
  const usage = proxy.usage_count;
  const { menuButtonRef, openDialog, onCloseAutoFocus } = useMenuDialog();
  const parts = [
    { content: proxy.scheme },
    { content: proxyAddress(proxy), className: 'font-mono' },
    ...(proxy.username ? [{ content: proxy.username }] : []),
    {
      content: usage > 0 ? t(i18n)`used by ${usage}` : t(i18n)`not used`,
    },
  ];
  return (
    <li aria-busy={actions.deleting || undefined}>
      <Disclosure open={open} onOpenChange={setOpen}>
        <div className="relative flex items-center gap-x-2 px-3 py-2.5 transition-colors hover:bg-muted/30 sm:gap-x-3 sm:px-4">
          <ProxyAvatar scheme={proxy.scheme} />
          <div className="min-w-0 flex-1">
            <p className="truncate text-sm font-medium">{proxy.name}</p>
            <p
              className="mt-0.5 line-clamp-2 text-xs text-muted-foreground @md:line-clamp-1"
              title={parts.map((part) => part.content).join(' · ')}
            >
              <DotSeparated parts={parts} />
            </p>
          </div>
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
              className={cn(ROW_BUTTON, 'relative z-10 hidden @md:inline-flex')}
              onClick={actions.onTest}
            >
              <Gauge className="h-3.5 w-3.5" />
              <Trans>Test</Trans>
            </Button>
          )}
          <RowExpandButton
            open={open}
            onToggle={() => {
              setOpen(!open);
              setExpanded(true);
            }}
            controls={detailsId}
            label={i18n._(msg`Details for ${proxy.name}`)}
          />
          <div className="relative z-10 shrink-0">
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button
                  ref={menuButtonRef}
                  type="button"
                  size="icon"
                  variant="ghost"
                  className="size-8 rounded-lg text-muted-foreground"
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
                <DropdownMenuItem onSelect={openDialog(actions.onTest)}>
                  <Gauge /> <Trans>Test</Trans>
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
        <DisclosureContent
          id={detailsId}
          className="space-y-2 pr-4 pb-3 pl-14 text-xs sm:pl-16"
        >
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
