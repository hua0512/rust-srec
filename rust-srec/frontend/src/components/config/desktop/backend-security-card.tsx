import {
  forwardRef,
  useEffect,
  useRef,
  useState,
  type ComponentType,
  type KeyboardEvent,
  type ReactNode,
} from 'react';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { toast } from 'sonner';
import {
  AlertTriangle,
  Check,
  CheckCircle2,
  Globe,
  Laptop,
  Loader2,
  RefreshCw,
  RotateCw,
  Server,
  XCircle,
} from 'lucide-react';

import { SetDesktopPasswordDialog } from '@/components/config/desktop/set-desktop-password-dialog';
import { SettingToggleRow } from '@/components/config/desktop/setting-toggle-row';
import { SettingsCard } from '@/components/config/settings-card';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Separator } from '@/components/ui/separator';
import {
  testRemoteBackendConnection,
  type DesktopSettingsSnapshot,
  type RemoteConnectionCheck,
} from '@/desktop/desktop-settings';
import { errorMessage } from '@/lib/api-error';
import { cn } from '@/lib/utils';
import type { DesktopBackendMode } from '@/utils/session-storage';

const LOOPBACK_IPV4 = /^127(?:\.\d{1,3}){3}$/;

/** True for an `http://` URL whose host is not this computer. */
function isInsecureRemoteUrl(value: string): boolean {
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    return false;
  }
  if (url.protocol !== 'http:') return false;
  const host = url.hostname.toLowerCase();
  return !(
    host === 'localhost' ||
    host === '[::1]' ||
    LOOPBACK_IPV4.test(host)
  );
}

interface BackendModeOptionProps {
  icon: ComponentType<{ className?: string; 'aria-hidden'?: 'true' }>;
  title: ReactNode;
  description: ReactNode;
  selected: boolean;
  active: boolean;
  disabled: boolean;
  onSelect: () => void;
  onKeyDown: (event: KeyboardEvent<HTMLButtonElement>) => void;
}

const BackendModeOption = forwardRef<HTMLButtonElement, BackendModeOptionProps>(
  function BackendModeOption(
    {
      icon: Icon,
      title,
      description,
      selected,
      active,
      disabled,
      onSelect,
      onKeyDown,
    },
    ref,
  ) {
    return (
      <button
        ref={ref}
        type="button"
        role="radio"
        aria-checked={selected}
        tabIndex={selected ? 0 : -1}
        disabled={disabled}
        onClick={onSelect}
        onKeyDown={onKeyDown}
        className={cn(
          'flex flex-col items-start gap-2 rounded-xl border p-3.5 text-left transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring',
          selected
            ? 'border-primary bg-primary/5 ring-1 ring-primary/30'
            : 'border-border/60 bg-muted/20 hover:bg-muted/40',
        )}
      >
        <div className="flex w-full items-center justify-between gap-2">
          <div className="flex items-center gap-2">
            <Icon
              className={cn(
                'h-4 w-4 shrink-0',
                selected ? 'text-primary' : 'text-muted-foreground',
              )}
              aria-hidden="true"
            />
            <span className="text-sm font-medium text-foreground">{title}</span>
          </div>
          <div className="flex items-center gap-1.5">
            {active && (
              <Badge
                variant="outline"
                className="border-emerald-500/30 bg-emerald-500/10 text-[11px] text-emerald-600 dark:text-emerald-400"
              >
                <Trans>Active</Trans>
              </Badge>
            )}
            <span
              aria-hidden="true"
              className={cn(
                'flex h-4 w-4 shrink-0 items-center justify-center rounded-full border',
                selected
                  ? 'border-primary bg-primary text-primary-foreground'
                  : 'border-muted-foreground/40',
              )}
            >
              {selected && <Check className="h-2.5 w-2.5" />}
            </span>
          </div>
        </div>
        <p className="text-xs leading-relaxed text-muted-foreground">
          {description}
        </p>
      </button>
    );
  },
);

interface RestartNoticeProps {
  message: ReactNode;
  buttonLabel: ReactNode;
  busy: boolean;
  onClick: () => void;
}

function RestartNotice({
  message,
  buttonLabel,
  busy,
  onClick,
}: RestartNoticeProps) {
  return (
    <div className="flex flex-col gap-3 rounded-xl border border-primary/30 bg-primary/5 p-3.5 sm:flex-row sm:items-center sm:justify-between">
      <p className="text-xs leading-relaxed text-muted-foreground">{message}</p>
      <Button
        type="button"
        size="sm"
        disabled={busy}
        onClick={onClick}
        className="shrink-0 gap-2 self-start sm:self-center"
      >
        {busy ? (
          <Loader2 className="h-3.5 w-3.5 animate-spin" aria-hidden="true" />
        ) : (
          <RotateCw className="h-3.5 w-3.5" aria-hidden="true" />
        )}
        {buttonLabel}
      </Button>
    </div>
  );
}

interface BackendSecurityCardProps {
  snapshot: DesktopSettingsSnapshot;
  isRequireLoginPending: boolean;
  onToggleRequireLogin: (checked: boolean) => void;
  /**
   * Sets the first password and turns password sign-in on; resolves to
   * `true` when the password was set.
   */
  onSetPasswordAndRequireLogin: (password: string) => Promise<boolean>;
  onApplyBackendSettings: (
    nextMode: DesktopBackendMode,
    nextRemoteUrl: string | null,
  ) => Promise<void>;
  /** Restarts into the saved backend without changing any setting. */
  onRestart: () => Promise<void>;
}

export function BackendSecurityCard({
  snapshot,
  isRequireLoginPending,
  onToggleRequireLogin,
  onSetPasswordAndRequireLogin,
  onApplyBackendSettings,
  onRestart,
}: BackendSecurityCardProps) {
  const { i18n } = useLingui();
  const [isPasswordDialogOpen, setIsPasswordDialogOpen] = useState(false);
  const [selectedMode, setSelectedMode] = useState<DesktopBackendMode>(
    snapshot.backendMode,
  );
  const [remoteUrlInput, setRemoteUrlInput] = useState(
    snapshot.remoteUrl ?? '',
  );
  const [connectionResult, setConnectionResult] =
    useState<RemoteConnectionCheck | null>(null);
  const [connectionError, setConnectionError] = useState<string | null>(null);
  const [isTestingConnection, setIsTestingConnection] = useState(false);
  const [isSavingBackend, setIsSavingBackend] = useState(false);

  useEffect(() => {
    setSelectedMode(snapshot.backendMode);
  }, [snapshot.backendMode]);

  useEffect(() => {
    setRemoteUrlInput(snapshot.remoteUrl ?? '');
    setConnectionResult(null);
    setConnectionError(null);
  }, [snapshot.remoteUrl]);

  const trimmedRemoteUrl = remoteUrlInput.trim();
  // A switch was saved but the restart did not happen (for example, it was
  // cancelled to keep a recording running). Hidden while another mode is
  // selected, whose own Save & Restart replaces it.
  const restartPending =
    snapshot.backendMode !== snapshot.activeBackendMode &&
    selectedMode === snapshot.backendMode;
  const needsRestartToLocal =
    selectedMode === 'local' && snapshot.backendMode !== 'local';
  const showInsecureUrlWarning = isInsecureRemoteUrl(trimmedRemoteUrl);

  // The shell refuses to require sign-in while the account still has its
  // initial password, so a password is asked for first.
  const handleRequireLoginChange = (checked: boolean) => {
    if (checked && snapshot.passwordSetupRequired) {
      setIsPasswordDialogOpen(true);
      return;
    }
    onToggleRequireLogin(checked);
  };

  const handleTestConnection = async () => {
    if (!trimmedRemoteUrl) return;
    setIsTestingConnection(true);
    setConnectionResult(null);
    setConnectionError(null);
    try {
      const result = await testRemoteBackendConnection(trimmedRemoteUrl);
      setConnectionResult(result);
    } catch (testError) {
      setConnectionError(
        errorMessage(
          testError,
          i18n._(msg`Unable to connect to the remote server.`),
        ),
      );
    } finally {
      setIsTestingConnection(false);
    }
  };

  const handleSaveAndRestart = async (mode: DesktopBackendMode) => {
    if (mode === 'remote' && !trimmedRemoteUrl) return;
    setIsSavingBackend(true);
    try {
      const nextUrl =
        mode === 'remote'
          ? connectionResult?.healthy
            ? connectionResult.normalizedUrl
            : trimmedRemoteUrl
          : snapshot.remoteUrl;
      await onApplyBackendSettings(mode, nextUrl);
    } catch (saveError) {
      toast.error(
        errorMessage(
          saveError,
          i18n._(msg`Failed to save backend settings and restart`),
        ),
      );
    } finally {
      setIsSavingBackend(false);
    }
  };

  const handleRestart = async () => {
    setIsSavingBackend(true);
    try {
      await onRestart();
    } catch (restartError) {
      toast.error(
        errorMessage(restartError, i18n._(msg`Failed to restart Rust-Srec`)),
      );
    } finally {
      setIsSavingBackend(false);
    }
  };

  const localRadioRef = useRef<HTMLButtonElement>(null);
  const remoteRadioRef = useRef<HTMLButtonElement>(null);

  const handleRadioKeyDown = (event: KeyboardEvent<HTMLButtonElement>) => {
    if (
      event.key !== 'ArrowLeft' &&
      event.key !== 'ArrowRight' &&
      event.key !== 'ArrowUp' &&
      event.key !== 'ArrowDown'
    ) {
      return;
    }
    event.preventDefault();
    const nextMode: DesktopBackendMode =
      selectedMode === 'local' ? 'remote' : 'local';
    setSelectedMode(nextMode);
    if (nextMode === 'local') {
      localRadioRef.current?.focus();
    } else {
      remoteRadioRef.current?.focus();
    }
  };

  return (
    <SettingsCard
      title={<Trans>Backend &amp; Security</Trans>}
      description={
        <Trans>
          Choose whether to run the built-in backend or connect to a remote
          Rust-Srec server.
        </Trans>
      }
      icon={Server}
      iconColor="text-sky-500"
      iconBgColor="bg-sky-500/10"
    >
      <div className="space-y-4">
        <div
          role="radiogroup"
          aria-label={i18n._(msg`Backend mode`)}
          className="grid grid-cols-1 gap-3 @md:grid-cols-2"
        >
          <BackendModeOption
            ref={localRadioRef}
            icon={Laptop}
            title={<Trans>Built-in local backend</Trans>}
            description={
              <Trans>
                Run the embedded Rust-Srec backend locally on this computer.
              </Trans>
            }
            selected={selectedMode === 'local'}
            active={snapshot.activeBackendMode === 'local'}
            disabled={isSavingBackend}
            onSelect={() => setSelectedMode('local')}
            onKeyDown={handleRadioKeyDown}
          />
          <BackendModeOption
            ref={remoteRadioRef}
            icon={Globe}
            title={<Trans>Remote rust-srec server</Trans>}
            description={
              <Trans>
                Connect the desktop app to an existing Rust-Srec server over
                HTTP or HTTPS.
              </Trans>
            }
            selected={selectedMode === 'remote'}
            active={snapshot.activeBackendMode === 'remote'}
            disabled={isSavingBackend}
            onSelect={() => setSelectedMode('remote')}
            onKeyDown={handleRadioKeyDown}
          />
        </div>

        {restartPending && (
          <RestartNotice
            message={
              snapshot.backendMode === 'local' ? (
                <Trans>
                  The built-in local backend is saved and takes effect after
                  Rust-Srec restarts.
                </Trans>
              ) : (
                <Trans>
                  The remote server is saved and takes effect after Rust-Srec
                  restarts.
                </Trans>
              )
            }
            buttonLabel={<Trans>Restart now</Trans>}
            busy={isSavingBackend}
            onClick={() => void handleRestart()}
          />
        )}

        <Separator className="opacity-50" />

        {selectedMode === 'local' ? (
          <div className="space-y-4">
            <SettingToggleRow
              id="desktop-require-login"
              label={<Trans>Require password sign-in</Trans>}
              description={
                <Trans>
                  Require entering your username and password to open the local
                  desktop app instead of signing in automatically.
                </Trans>
              }
              checked={snapshot.requireLogin}
              disabled={isRequireLoginPending || isSavingBackend}
              onCheckedChange={handleRequireLoginChange}
            />
            <SetDesktopPasswordDialog
              open={isPasswordDialogOpen}
              onOpenChange={setIsPasswordDialogOpen}
              username={snapshot.localUsername}
              onSubmit={onSetPasswordAndRequireLogin}
            />

            {needsRestartToLocal && (
              <RestartNotice
                message={
                  <Trans>
                    Switching back to the built-in local backend requires
                    restarting Rust-Srec.
                  </Trans>
                }
                buttonLabel={<Trans>Save &amp; Restart</Trans>}
                busy={isSavingBackend}
                onClick={() => void handleSaveAndRestart('local')}
              />
            )}
          </div>
        ) : (
          <div className="space-y-3">
            <div className="space-y-1.5">
              <label
                htmlFor="desktop-remote-url"
                className="block text-sm font-medium text-foreground"
              >
                <Trans>Server URL</Trans>
              </label>
              <Input
                id="desktop-remote-url"
                type="url"
                value={remoteUrlInput}
                disabled={isSavingBackend}
                autoComplete="off"
                spellCheck={false}
                placeholder="https://srec.example.com"
                onChange={(event) => {
                  setRemoteUrlInput(event.target.value);
                  setConnectionResult(null);
                  setConnectionError(null);
                }}
              />
              {showInsecureUrlWarning && (
                <p className="flex items-start gap-1.5 text-xs leading-relaxed text-amber-600 dark:text-amber-400">
                  <AlertTriangle
                    className="mt-0.5 h-3.5 w-3.5 shrink-0"
                    aria-hidden="true"
                  />
                  <Trans>
                    This address uses http://, so your password and sign-in
                    tokens are sent unencrypted.
                  </Trans>
                </p>
              )}
            </div>

            <div className="flex flex-wrap items-center gap-2">
              <Button
                type="button"
                variant="outline"
                size="sm"
                disabled={
                  !trimmedRemoteUrl || isTestingConnection || isSavingBackend
                }
                onClick={() => void handleTestConnection()}
                className="gap-2"
              >
                {isTestingConnection ? (
                  <Loader2
                    className="h-3.5 w-3.5 animate-spin"
                    aria-hidden="true"
                  />
                ) : (
                  <RefreshCw className="h-3.5 w-3.5" aria-hidden="true" />
                )}
                <Trans>Test connection</Trans>
              </Button>

              <Button
                type="button"
                size="sm"
                disabled={
                  !trimmedRemoteUrl || isSavingBackend || isTestingConnection
                }
                onClick={() => void handleSaveAndRestart('remote')}
                className="gap-2"
              >
                {isSavingBackend ? (
                  <Loader2
                    className="h-3.5 w-3.5 animate-spin"
                    aria-hidden="true"
                  />
                ) : (
                  <RotateCw className="h-3.5 w-3.5" aria-hidden="true" />
                )}
                <Trans>Save &amp; Restart</Trans>
              </Button>
            </div>

            {connectionResult && (
              <div className="flex flex-wrap items-center gap-2 rounded-xl border border-border/50 bg-muted/20 p-3 text-xs">
                {connectionResult.healthy ? (
                  <Badge
                    variant="outline"
                    className="border-emerald-500/30 bg-emerald-500/10 text-emerald-600 dark:text-emerald-400"
                  >
                    <CheckCircle2 className="h-3 w-3" aria-hidden="true" />
                    <Trans>Connected</Trans>
                  </Badge>
                ) : (
                  <Badge variant="destructive">
                    <XCircle className="h-3 w-3" aria-hidden="true" />
                    <Trans>Connection failed</Trans>
                  </Badge>
                )}
                <code className="min-w-0 truncate font-mono text-muted-foreground">
                  {connectionResult.normalizedUrl}
                </code>
                {connectionResult.statusCode > 0 && (
                  <Badge variant="outline" className="font-mono text-[11px]">
                    HTTP {connectionResult.statusCode}
                  </Badge>
                )}
                {connectionResult.detail && (
                  <span className="text-muted-foreground">
                    {connectionResult.detail}
                  </span>
                )}
              </div>
            )}

            {connectionError && (
              <div className="flex flex-wrap items-center gap-2 rounded-xl border border-destructive/30 bg-destructive/5 p-3 text-xs">
                <Badge variant="destructive">
                  <XCircle className="h-3 w-3" aria-hidden="true" />
                  <Trans>Connection failed</Trans>
                </Badge>
                <span className="text-destructive">{connectionError}</span>
              </div>
            )}
          </div>
        )}
      </div>
    </SettingsCard>
  );
}
