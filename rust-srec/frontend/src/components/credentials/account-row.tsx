import { useId, useState } from 'react';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import type { MessageDescriptor } from '@lingui/core';
import {
  Copy,
  Loader2,
  MoreHorizontal,
  Network,
  Pencil,
  Power,
  QrCode,
  RefreshCw,
  ShieldCheck,
  Trash2,
} from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Disclosure, DisclosureContent } from '@/components/ui/disclosure';
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from '@/components/ui/tooltip';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import { useMenuDialog } from '@/hooks/use-menu-dialog';
import { cn } from '@/lib/utils';
import type { CredentialProfileDetail } from '@/api/schemas/credential-profiles';
import { attentionReason, VALIDITY_STYLES } from './account-health';
import { AccountAvatar } from './account-avatar';
import { AccountDetails, AccountSummaryLine } from './account-usage';
import { ROW_BUTTON, RowExpandButton } from '@/components/shared/list-panel';
import {
  routeName,
  useSavedProxies,
} from '@/components/proxies/proxy-route-label';

export type AccountAction = 'validate' | 'refresh' | 'toggle';

/** Anything that can be running on one account. */
export type AccountActivity = AccountAction | 'delete';

/**
 * The button a row offers beside its menu: a new login for an enabled account
 * whose health calls for one, by QR code where the platform supports it and
 * otherwise by pasting new credentials. Disabled accounts are left alone, as
 * in the header's list.
 */
export function contextualAction(
  detail: CredentialProfileDetail,
): 'qr_login' | 'edit' | undefined {
  if (!detail.profile.enabled || !attentionReason(detail.health))
    return undefined;
  return detail.capabilities.qr_login ? 'qr_login' : 'edit';
}

const ACTIVITY_LABELS: Record<AccountActivity, MessageDescriptor> = {
  validate: msg`Checking…`,
  refresh: msg`Refreshing…`,
  toggle: msg`Saving…`,
  delete: msg`Deleting…`,
};

/** The management controls of a row on the platform page. */
interface AccountRowActions {
  /** What is running on this account; other rows stay usable meanwhile. */
  pending?: AccountActivity;
  onAction: (type: AccountAction) => void;
  /** `replace` opens the editor with credential replacement switched on. */
  onEdit: (replace?: boolean) => void;
  onDelete: () => void;
  onCopyId: () => void;
  onQrLogin: () => void;
}

/**
 * The quiet word beside an account that needs nothing done: its health, or
 * that it is disabled. Nothing is said of an unchecked account on a platform
 * that cannot check accounts.
 */
function StatusWord({ detail }: { detail: CredentialProfileDetail }) {
  const { i18n } = useLingui();
  if (!detail.profile.enabled)
    return (
      <span className="text-muted-foreground">
        <Trans>Disabled</Trans>
      </span>
    );
  const validity = detail.health?.validity ?? 'unknown';
  if (validity === 'unknown' && !detail.capabilities.validate) return null;
  const style = VALIDITY_STYLES[validity];
  return <span className={style.text}>{i18n._(style.label)}</span>;
}

/**
 * One account: its avatar with health, its name and a one-line summary, and
 * the details under it when expanded. Read-only unless `actions` is given, in
 * which case the row also offers the contextual login and its menu.
 */
export function AccountRow({
  detail,
  actions,
}: {
  detail: CredentialProfileDetail;
  actions?: AccountRowActions;
}) {
  const { i18n } = useLingui();
  const [open, setOpen] = useState(false);
  const detailsId = useId();
  const profile = detail.profile;
  const route = profile.proxy_route;
  const proxies = useSavedProxies().data;
  const routeLabel =
    route && route.kind !== 'inherit'
      ? routeName(i18n, route, proxies)
      : undefined;
  const pending = actions?.pending;
  const busy = pending !== undefined;
  const contextual = actions && contextualAction(detail);
  const { menuButtonRef, openDialog, onCloseAutoFocus } = useMenuDialog();
  return (
    <li aria-busy={busy || undefined}>
      <Disclosure open={open} onOpenChange={setOpen}>
        {/* The chevron's hit area stretches over the whole row, so a click
            anywhere but on the actions expands it; the actions sit above. */}
        <div className="relative flex flex-wrap items-center gap-x-2 gap-y-2 px-3 py-2.5 transition-colors hover:bg-muted/30 sm:gap-x-3 sm:px-4">
          <AccountAvatar
            label={profile.label}
            validity={detail.health?.validity}
            enabled={profile.enabled}
          />
          <div className="min-w-0 flex-1">
            <div className="flex min-w-0 items-center gap-1.5">
              <span
                className={cn(
                  'truncate text-sm font-medium',
                  !profile.enabled && 'text-muted-foreground',
                )}
              >
                {profile.label}
              </span>
              {routeLabel && (
                <Tooltip>
                  <TooltipTrigger asChild>
                    <span className="relative z-10 shrink-0 cursor-help text-muted-foreground">
                      <Network
                        className="size-3.5"
                        role="img"
                        aria-label={i18n._(msg`Own proxy setting`)}
                      />
                    </span>
                  </TooltipTrigger>
                  <TooltipContent>
                    {route?.kind === 'direct' ? (
                      <Trans>Requests for this account connect directly.</Trans>
                    ) : route?.kind === 'system' ? (
                      <Trans>
                        Requests for this account use the system proxy.
                      </Trans>
                    ) : (
                      <Trans>
                        Requests for this account go through {routeLabel}.
                      </Trans>
                    )}
                  </TooltipContent>
                </Tooltip>
              )}
            </div>
            <AccountSummaryLine
              detail={detail}
              className="mt-0.5 text-xs text-muted-foreground line-clamp-2 @md:line-clamp-1"
            />
          </div>
          {!contextual && !busy && (
            // Narrow rows leave the health to the avatar's dot.
            <span className="sr-only shrink-0 text-xs font-medium @md:not-sr-only">
              <StatusWord detail={detail} />
            </span>
          )}
          {actions && (busy || contextual) && (
            // Narrow rows give the action a line of its own under the text,
            // so it never squeezes the name.
            <div className="relative z-10 order-last flex basis-full items-center gap-1.5 pl-11 @md:order-none @md:basis-auto @md:pl-0">
              {pending && (
                <span
                  role="status"
                  className="flex items-center gap-1.5 px-1 text-xs text-muted-foreground"
                >
                  <Loader2 className="h-3.5 w-3.5 animate-spin" />
                  {i18n._(ACTIVITY_LABELS[pending])}
                </span>
              )}
              {contextual === 'qr_login' && (
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  className={ROW_BUTTON}
                  disabled={busy}
                  onClick={actions.onQrLogin}
                >
                  <QrCode className="h-3.5 w-3.5" />
                  <Trans>Log in again</Trans>
                </Button>
              )}
              {contextual === 'edit' && (
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  className={ROW_BUTTON}
                  disabled={busy}
                  onClick={() => actions.onEdit(true)}
                >
                  <Pencil className="h-3.5 w-3.5" />
                  <Trans>Edit</Trans>
                </Button>
              )}
            </div>
          )}
          <RowExpandButton
            open={open}
            onToggle={() => setOpen(!open)}
            controls={detailsId}
            label={i18n._(msg`Details for ${profile.label}`)}
          />
          {actions && (
            <div className="relative z-10 shrink-0">
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <Button
                    ref={menuButtonRef}
                    type="button"
                    size="icon"
                    variant="ghost"
                    className="size-8 rounded-lg text-muted-foreground"
                    aria-label={i18n._(msg`More actions for ${profile.label}`)}
                  >
                    <MoreHorizontal className="h-4 w-4" />
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent
                  align="end"
                  className="w-52 rounded-xl"
                  onCloseAutoFocus={onCloseAutoFocus}
                >
                  {detail.capabilities.validate && (
                    <DropdownMenuItem
                      disabled={busy || !profile.enabled}
                      onSelect={() => actions.onAction('validate')}
                    >
                      <ShieldCheck /> <Trans>Validate</Trans>
                    </DropdownMenuItem>
                  )}
                  {detail.capabilities.qr_login && (
                    <DropdownMenuItem
                      disabled={busy}
                      onSelect={openDialog(actions.onQrLogin)}
                    >
                      <QrCode /> <Trans>Log in with QR code</Trans>
                    </DropdownMenuItem>
                  )}
                  {detail.capabilities.refresh && (
                    <DropdownMenuItem
                      disabled={busy || !profile.enabled}
                      onSelect={() => actions.onAction('refresh')}
                    >
                      <RefreshCw /> <Trans>Refresh</Trans>
                    </DropdownMenuItem>
                  )}
                  <DropdownMenuItem
                    onSelect={openDialog(() => actions.onEdit())}
                  >
                    <Pencil /> <Trans>Edit</Trans>
                  </DropdownMenuItem>
                  <DropdownMenuItem
                    disabled={busy}
                    onSelect={() => actions.onAction('toggle')}
                  >
                    <Power />{' '}
                    {profile.enabled ? (
                      <Trans>Disable</Trans>
                    ) : (
                      <Trans>Enable</Trans>
                    )}
                  </DropdownMenuItem>
                  <DropdownMenuItem onSelect={actions.onCopyId}>
                    <Copy /> <Trans>Copy ID</Trans>
                  </DropdownMenuItem>
                  <DropdownMenuSeparator />
                  <DropdownMenuItem
                    variant="destructive"
                    disabled={busy}
                    onSelect={openDialog(actions.onDelete)}
                  >
                    <Trash2 /> <Trans>Delete</Trans>
                  </DropdownMenuItem>
                </DropdownMenuContent>
              </DropdownMenu>
            </div>
          )}
        </div>
        <DisclosureContent id={detailsId} className="pr-4 pb-3 pl-14 sm:pl-16">
          <AccountDetails detail={detail} />
        </DisclosureContent>
      </Disclosure>
    </li>
  );
}
