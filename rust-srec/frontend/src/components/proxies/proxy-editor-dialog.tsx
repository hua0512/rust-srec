import { useId, useRef, useState, type ReactNode } from 'react';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { AlertCircle, Globe, Lock, Tag, User, X } from 'lucide-react';
import { Callout } from '@/components/shared/callout';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { Separator } from '@/components/ui/separator';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import {
  CONFIG_DESCRIPTION,
  CONFIG_INPUT,
} from '@/components/config/shared/config-field';
import { cn } from '@/lib/utils';
import { createProxy, updateProxy } from '@/server/functions/proxies';
import { invalidateProxyQueries } from '@/api/proxies';
import type { SavedProxy } from '@/api/schemas/proxies';
import { proxyConflict } from './proxy-errors';
import { useSavedProxies } from './proxy-route-label';
import { ProxyTestPanel, type ProxyTestEndpoint } from './proxy-test';

const SCHEMES = ['http', 'https', 'socks5', 'socks5h'];

/**
 * The address as saved: a bare `host:port` is an http proxy, as browsers and
 * most tools read it.
 */
export function normalizeProxyUrl(input: string): string {
  const trimmed = input.trim();
  if (!trimmed || trimmed.includes('://')) return trimmed;
  return `http://${trimmed}`;
}

type AddressProblem = 'invalid' | 'scheme' | 'login' | 'path';

/** What is wrong with an address the backend would refuse, if anything. */
function addressProblem(url: string): AddressProblem | undefined {
  let parsed: URL;
  try {
    parsed = new URL(url);
  } catch {
    return 'invalid';
  }
  if (!SCHEMES.includes(parsed.protocol.replace(/:$/, ''))) return 'scheme';
  if (!parsed.hostname) return 'invalid';
  if (parsed.username || parsed.password) return 'login';
  if (parsed.search || parsed.hash || !['', '/'].includes(parsed.pathname))
    return 'path';
  return undefined;
}

const ADDRESS_PROBLEMS = {
  invalid: msg`Enter an address such as http://proxy.example:8080.`,
  scheme: msg`Use an http, https, socks5 or socks5h address.`,
  login: msg`Put the login in the username and password fields instead.`,
  path: msg`Leave out any path: only scheme://host:port.`,
} as const;

/**
 * Adds a saved proxy or edits one. A saved password is never shown: editing
 * keeps it unless a new one is typed, and clearing the username removes the
 * login. The proxy can be checked before saving.
 */
export function ProxyEditorDialog({
  proxy,
  onClose,
  onSaved,
  onUseExisting,
}: {
  /** The proxy to edit; omitted to add one. */
  proxy?: SavedProxy;
  onClose: () => void;
  onSaved: (saved: SavedProxy) => void;
  /**
   * Offered when another proxy already reaches the same address with the
   * same username, to use that one instead.
   */
  onUseExisting?: (existing: SavedProxy) => void;
}) {
  const { i18n } = useLingui();
  const queryClient = useQueryClient();
  const proxies = useSavedProxies().data;
  const nameInput = useRef<HTMLInputElement>(null);
  const ids = { name: useId(), url: useId(), user: useId(), pass: useId() };
  const [name, setName] = useState(proxy?.name ?? '');
  const [address, setAddress] = useState(proxy?.url ?? '');
  const [username, setUsername] = useState(proxy?.username ?? '');
  // A saved password is never shown, so the field always starts empty.
  const [password, setPassword] = useState('');
  const url = normalizeProxyUrl(address);
  const problem = url ? addressProblem(url) : undefined;
  const hadLogin = Boolean(proxy?.username);
  const removesLogin = hadLogin && !username;
  const keepsPassword = Boolean(proxy?.has_password) && !removesLogin;

  /** The edit as the backend reads it: only what changed. */
  const changes = () => {
    if (!proxy) return undefined;
    const login =
      username === (proxy.username ?? '')
        ? {}
        : { username: username ? username : null };
    // A login on a proxy without a saved password needs one, even empty.
    const newPassword =
      password || (username && !proxy.has_password) ? { password } : {};
    return {
      ...(name.trim() !== proxy.name ? { name: name.trim() } : {}),
      ...(url !== proxy.url ? { url } : {}),
      ...login,
      ...(username ? newPassword : {}),
    };
  };

  const save = useMutation({
    mutationFn: async () => {
      if (proxy)
        return updateProxy({
          data: {
            id: proxy.id,
            expected_version: proxy.version,
            ...changes(),
          },
        });
      return createProxy({
        data: {
          name: name.trim(),
          url,
          ...(username ? { username, password } : {}),
        },
      });
    },
    onSuccess: (saved) => {
      void invalidateProxyQueries(queryClient);
      onSaved(saved);
    },
  });

  const testEndpoint = (): ProxyTestEndpoint | undefined => {
    if (!url || problem) return undefined;
    if (!proxy)
      return {
        url,
        ...(username ? { username, password } : {}),
      };
    const edit = changes() ?? {};
    return {
      proxy_id: proxy.id,
      ...('url' in edit ? { url: edit.url } : {}),
      ...('username' in edit ? { username: edit.username } : {}),
      ...('password' in edit ? { password: edit.password } : {}),
    };
  };

  const conflict = save.error ? proxyConflict(save.error) : undefined;
  const existing =
    conflict?.kind === 'duplicate'
      ? proxies?.find((candidate) => candidate.name === conflict.name)
      : undefined;
  const conflictName = conflict && 'name' in conflict ? conflict.name : '';
  let error: ReactNode = save.error?.message;
  if (conflict?.kind === 'name_taken')
    error = <Trans>Another proxy is already named “{conflictName}”.</Trans>;
  else if (conflict?.kind === 'duplicate')
    error = (
      <Trans>“{conflictName}” already uses this address and username.</Trans>
    );
  else if (conflict?.kind === 'stale')
    error = (
      <Trans>
        This proxy was changed elsewhere. Close the editor and open it again.
      </Trans>
    );

  const canSave = Boolean(name.trim() && url && !problem) && !save.isPending;
  const normalized = url !== address.trim() && url !== '' ? url : undefined;

  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
    >
      <DialogContent
        className="max-h-[90vh] overflow-y-auto border-border/50 sm:max-w-lg"
        onOpenAutoFocus={(event) => {
          const input = nameInput.current;
          if (!input) return;
          event.preventDefault();
          input.focus();
          input.setSelectionRange(input.value.length, input.value.length);
        }}
      >
        <DialogHeader>
          <DialogTitle>
            {proxy ? <Trans>Edit proxy</Trans> : <Trans>Add proxy</Trans>}
          </DialogTitle>
          <DialogDescription>
            {proxy ? (
              <Trans>
                Changes apply to the next connection everywhere this proxy is
                used.
              </Trans>
            ) : (
              <Trans>
                Saved proxies can be chosen in global, platform, template,
                streamer and account settings.
              </Trans>
            )}
          </DialogDescription>
        </DialogHeader>
        {/* The dialog renders inside configuration forms; its own form keeps
            Enter here from submitting them. */}
        <form
          className="space-y-4"
          onSubmit={(event) => {
            event.preventDefault();
            event.stopPropagation();
            if (canSave) save.mutate();
          }}
        >
          <Field id={ids.name} icon={Tag} label={<Trans>Name</Trans>}>
            <Input
              id={ids.name}
              ref={nameInput}
              className={CONFIG_INPUT}
              value={name}
              maxLength={128}
              placeholder={i18n._(msg`Office proxy`)}
              onChange={(event) => setName(event.target.value)}
            />
          </Field>
          <Field
            id={ids.url}
            icon={Globe}
            label={<Trans>Address</Trans>}
            description={
              problem ? (
                <span className="text-destructive">
                  {i18n._(ADDRESS_PROBLEMS[problem])}
                </span>
              ) : normalized ? (
                <Trans>Saved as {normalized}</Trans>
              ) : (
                <Trans>http, https, socks5 or socks5h.</Trans>
              )
            }
          >
            <Input
              id={ids.url}
              autoComplete="off"
              spellCheck={false}
              className={cn(CONFIG_INPUT, 'font-mono text-xs')}
              placeholder="http://proxy.example:8080"
              value={address}
              aria-invalid={problem ? true : undefined}
              onChange={(event) => setAddress(event.target.value)}
            />
          </Field>
          <div className="@container">
            <div className="grid grid-cols-1 gap-4 @md:grid-cols-2">
              <Field
                id={ids.user}
                icon={User}
                label={<Trans>Username</Trans>}
                action={
                  hadLogin && username ? (
                    <Button
                      type="button"
                      variant="ghost"
                      size="sm"
                      className="h-6 gap-1 rounded-md px-1.5 text-[11px] text-muted-foreground"
                      onClick={() => {
                        setUsername('');
                        setPassword('');
                      }}
                    >
                      <X className="size-3" />
                      <Trans>Remove login</Trans>
                    </Button>
                  ) : undefined
                }
                description={
                  removesLogin ? (
                    <Trans>The saved login will be removed.</Trans>
                  ) : (
                    <Trans>Optional.</Trans>
                  )
                }
              >
                <Input
                  id={ids.user}
                  autoComplete="off"
                  className={CONFIG_INPUT}
                  value={username}
                  onChange={(event) => setUsername(event.target.value)}
                />
              </Field>
              <Field
                id={ids.pass}
                icon={Lock}
                label={<Trans>Password</Trans>}
                description={
                  keepsPassword ? (
                    <Trans>Leave empty to keep the current password.</Trans>
                  ) : undefined
                }
              >
                <Input
                  id={ids.pass}
                  type="password"
                  autoComplete="new-password"
                  className={cn(CONFIG_INPUT, 'font-mono text-xs')}
                  disabled={!username}
                  placeholder={keepsPassword ? '••••••••' : undefined}
                  value={password}
                  onChange={(event) => setPassword(event.target.value)}
                />
              </Field>
            </div>
          </div>
          <Separator />
          <ProxyTestPanel endpoint={testEndpoint} />
          {save.error && (
            <Callout
              tone="error"
              icon={AlertCircle}
              action={
                existing &&
                onUseExisting && (
                  <Button
                    type="button"
                    size="sm"
                    variant="outline"
                    className="mt-2 h-7 rounded-lg text-xs"
                    onClick={() => onUseExisting(existing)}
                  >
                    <Trans>Use “{conflictName}” instead</Trans>
                  </Button>
                )
              }
            >
              {error}
            </Callout>
          )}
          <DialogFooter>
            <Button type="button" variant="outline" onClick={onClose}>
              <Trans>Cancel</Trans>
            </Button>
            <Button type="submit" disabled={!canSave}>
              {proxy ? <Trans>Save changes</Trans> : <Trans>Add proxy</Trans>}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

function Field({
  id,
  icon: Icon,
  label,
  action,
  description,
  children,
}: {
  id: string;
  icon: typeof Globe;
  label: ReactNode;
  action?: ReactNode;
  description?: ReactNode;
  children: ReactNode;
}) {
  return (
    <div className="space-y-2">
      <div className="flex min-h-6 items-center justify-between gap-2 px-1">
        <Label
          htmlFor={id}
          className="flex items-center gap-2 text-xs font-semibold text-muted-foreground"
        >
          <Icon className="size-3.5 text-primary" aria-hidden="true" />
          {label}
        </Label>
        {action}
      </div>
      {children}
      {description && <p className={CONFIG_DESCRIPTION}>{description}</p>}
    </div>
  );
}
