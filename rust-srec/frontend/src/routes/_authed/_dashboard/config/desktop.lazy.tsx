import { useEffect, useRef, useState, type ReactNode } from 'react';
import { createLazyFileRoute, useRouter } from '@tanstack/react-router';
import {
  useMutation,
  useMutationState,
  useQuery,
  useQueryClient,
  type QueryClient,
} from '@tanstack/react-query';
import { motion } from 'motion/react';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { toast } from 'sonner';
import {
  AlertCircle,
  ExternalLink,
  FolderOpen,
  Info,
  Loader2,
  Monitor,
  RefreshCw,
  ShieldCheck,
} from 'lucide-react';

import { BackendSecurityCard } from '@/components/config/desktop/backend-security-card';
import { ExternalToolsCard } from '@/components/config/desktop/external-tools-card';
import { SettingToggleRow } from '@/components/config/desktop/setting-toggle-row';
import { UpdatesCard } from '@/components/config/desktop/updates-card';
import { SettingsCard } from '@/components/config/settings-card';
import { SettingsCardSkeleton } from '@/components/config/settings-card-skeleton';
import { Button } from '@/components/ui/button';
import { Separator } from '@/components/ui/separator';
import { clearDesktopAutoLoginSessionFn } from '@/server/functions/auth';
import { endDesktopBackendSession } from '@/desktop/desktop-backend-switch';
import {
  getDesktopSettings,
  listenDesktopSettingsUpdated,
  openDesktopRecordingsFolder,
  openDesktopSystemFolder,
  restartDesktopApp,
  setDesktopAutostartEnabled,
  setDesktopPassword,
  skipDesktopUpdateVersion,
  updateDesktopSettings,
  type DesktopSettings,
  type DesktopSettingsSnapshot,
} from '@/desktop/desktop-settings';
import { errorMessage } from '@/lib/api-error';
import { containerVariants, itemVariants } from '@/lib/animation';
import { isTauriRuntime } from '@/utils/tauri';
import type { DesktopBackendMode } from '@/utils/session-storage';

const DESKTOP_SETTINGS_QUERY_KEY = ['desktop', 'settings'] as const;
const DESKTOP_SETTINGS_CHANGE_KEY = ['desktop', 'settings', 'change'] as const;
// `update_desktop_settings` replaces the whole settings file and Tauri runs
// commands concurrently, so every settings write shares this scope, which
// makes TanStack Query run them one at a time. Each write is then built from
// the shell's reply to the one before it.
const DESKTOP_SETTINGS_WRITE_SCOPE = { id: 'desktop-settings' };

type DesktopBooleanSettingKey =
  | 'startMinimized'
  | 'closeToTray'
  | 'minimizeToTray'
  | 'preventSleepWhileRecording'
  | 'confirmExitWhileRecording'
  | 'requireLogin'
  | 'checkUpdatesOnStartup';

type DesktopSettingsChange = Partial<DesktopSettings>;

type FolderActionKey = 'recordings' | 'data' | 'logs';

export const Route = createLazyFileRoute('/_authed/_dashboard/config/desktop')({
  component: DesktopSettingsPage,
});

function extractDesktopSettings(
  snapshot: DesktopSettingsSnapshot,
): DesktopSettings {
  return {
    startMinimized: snapshot.startMinimized,
    closeToTray: snapshot.closeToTray,
    minimizeToTray: snapshot.minimizeToTray,
    preventSleepWhileRecording: snapshot.preventSleepWhileRecording,
    confirmExitWhileRecording: snapshot.confirmExitWhileRecording,
    requireLogin: snapshot.requireLogin,
    checkUpdatesOnStartup: snapshot.checkUpdatesOnStartup,
    skippedUpdateVersion: snapshot.skippedUpdateVersion,
    backendMode: snapshot.backendMode,
    remoteUrl: snapshot.remoteUrl,
  };
}

/**
 * The settings as the shell last reported them, without optimistic values.
 * Writes are built from this rather than from the query cache, which may hold
 * toggles whose own write is still queued or about to fail. It lives outside
 * the page component because a write can settle after the page was left and
 * opened again, and the page that is open then must see its outcome.
 */
const confirmedDesktopSettings: { current: DesktopSettings | null } = {
  current: null,
};

/** Toggle changes that are queued or in flight, oldest first. */
function pendingSettingsChanges(
  queryClient: QueryClient,
  except?: DesktopSettingsChange,
): DesktopSettingsChange[] {
  return queryClient
    .getMutationCache()
    .findAll({ mutationKey: DESKTOP_SETTINGS_CHANGE_KEY, status: 'pending' })
    .map((mutation) => mutation.state.variables as DesktopSettingsChange)
    .filter((change) => change !== except);
}

/**
 * The shell's settings with the optimistic values of toggles that have not
 * been written yet laid over them, so a reply or event for one write does not
 * flip a switch the user changed after it.
 */
function mergeDesktopSettingsSnapshot(
  prev: DesktopSettingsSnapshot | undefined,
  confirmed: DesktopSettings,
  pending: readonly DesktopSettingsChange[],
): DesktopSettingsSnapshot | undefined {
  if (!prev) return prev;
  return Object.assign({ ...prev, ...confirmed }, ...pending);
}

interface FolderRowProps {
  label: ReactNode;
  path: string | null;
  emptyLabel: ReactNode;
  isOpening: boolean;
  onOpen: () => void;
}

function FolderRow({
  label,
  path,
  emptyLabel,
  isOpening,
  onOpen,
}: FolderRowProps) {
  return (
    <div className="flex flex-col gap-3 rounded-xl border border-border/50 bg-muted/20 p-3.5 sm:flex-row sm:items-center sm:justify-between">
      <div className="min-w-0 flex-1 space-y-1">
        <p className="text-sm font-medium leading-none text-foreground">
          {label}
        </p>
        {path ? (
          <code
            title={path}
            className="block min-w-0 truncate font-mono text-xs text-muted-foreground"
          >
            {path}
          </code>
        ) : (
          <p className="text-xs italic text-muted-foreground/70">
            {emptyLabel}
          </p>
        )}
      </div>
      <Button
        type="button"
        variant="outline"
        size="sm"
        disabled={!path || isOpening}
        onClick={onOpen}
        className="shrink-0 gap-2 self-start sm:self-center"
      >
        {isOpening ? (
          <Loader2 className="h-3.5 w-3.5 animate-spin" aria-hidden="true" />
        ) : (
          <ExternalLink className="h-3.5 w-3.5" aria-hidden="true" />
        )}
        <Trans>Open folder</Trans>
      </Button>
    </div>
  );
}

function DesktopUnavailableNotice() {
  return (
    <motion.div
      variants={containerVariants}
      initial="hidden"
      animate="visible"
      className="max-w-3xl"
    >
      <motion.div variants={itemVariants}>
        <SettingsCard
          title={<Trans>Desktop</Trans>}
          description={<Trans>Tray, startup &amp; window behavior</Trans>}
          icon={Monitor}
        >
          <div className="flex items-start gap-3 rounded-xl border border-dashed border-border/70 bg-muted/30 p-4 text-sm text-muted-foreground">
            <Info
              className="mt-0.5 h-4 w-4 shrink-0 text-primary"
              aria-hidden="true"
            />
            <p className="leading-relaxed">
              <Trans>
                Desktop preferences are only available in the Rust-Srec desktop
                app. Open Rust-Srec on your computer to configure tray,
                autostart, sleep protection, and local folder shortcuts.
              </Trans>
            </p>
          </div>
        </SettingsCard>
      </motion.div>
    </motion.div>
  );
}

function DesktopSettingsPage() {
  if (!isTauriRuntime()) {
    return <DesktopUnavailableNotice />;
  }

  return <DesktopSettingsContent />;
}

function DesktopSettingsContent() {
  const { i18n } = useLingui();
  const queryClient = useQueryClient();
  const router = useRouter();

  const [openingFolder, setOpeningFolder] = useState<FolderActionKey | null>(
    null,
  );

  const applyConfirmedSettings = (
    confirmed: DesktopSettings,
    except?: DesktopSettingsChange,
  ) => {
    confirmedDesktopSettings.current = confirmed;
    queryClient.setQueryData<DesktopSettingsSnapshot>(
      DESKTOP_SETTINGS_QUERY_KEY,
      (prev) =>
        mergeDesktopSettingsSnapshot(
          prev,
          confirmed,
          pendingSettingsChanges(queryClient, except),
        ),
    );
  };

  const writeDesktopSettings = (change: DesktopSettingsChange) => {
    const confirmed = confirmedDesktopSettings.current;
    if (!confirmed) {
      return Promise.reject(new Error('Desktop settings have not loaded yet.'));
    }
    return updateDesktopSettings({ ...confirmed, ...change });
  };

  const {
    data: snapshot,
    isLoading,
    isError,
    error,
    refetch,
  } = useQuery({
    queryKey: DESKTOP_SETTINGS_QUERY_KEY,
    queryFn: async () => {
      const loaded = await getDesktopSettings();
      confirmedDesktopSettings.current = extractDesktopSettings(loaded);
      return loaded;
    },
    // A local shell command either answers or fails for good, and the
    // settings-updated event below keeps the cache current, so a focus
    // refetch would only risk overwriting an optimistic toggle.
    retry: false,
    refetchOnWindowFocus: false,
  });

  const pendingChanges = useMutationState({
    filters: { mutationKey: DESKTOP_SETTINGS_CHANGE_KEY, status: 'pending' },
    select: (mutation) => mutation.state.variables as DesktopSettingsChange,
  });
  const isSettingPending = (key: DesktopBooleanSettingKey) =>
    pendingChanges.some((change) => key in change);

  const applyConfirmedSettingsRef = useRef(applyConfirmedSettings);
  applyConfirmedSettingsRef.current = applyConfirmedSettings;

  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | undefined;

    void listenDesktopSettingsUpdated((updated) => {
      applyConfirmedSettingsRef.current(updated);
    })
      .then((cleanup) => {
        if (active) {
          unlisten = cleanup;
        } else {
          cleanup();
        }
      })
      .catch(() => {
        // Ignore listener setup errors if the webview is tearing down.
      });

    return () => {
      active = false;
      unlisten?.();
    };
  }, []);

  const settingsMutation = useMutation({
    mutationKey: DESKTOP_SETTINGS_CHANGE_KEY,
    scope: DESKTOP_SETTINGS_WRITE_SCOPE,
    mutationFn: writeDesktopSettings,
    onMutate: async (change) => {
      await queryClient.cancelQueries({ queryKey: DESKTOP_SETTINGS_QUERY_KEY });
      queryClient.setQueryData<DesktopSettingsSnapshot>(
        DESKTOP_SETTINGS_QUERY_KEY,
        (prev) => (prev ? { ...prev, ...change } : prev),
      );
    },
    onSuccess: (updated, change) => {
      applyConfirmedSettings(updated);

      if (change.requireLogin === true && updated.requireLogin) {
        clearDesktopAutoLoginSessionFn()
          .then(async (cleared) => {
            if (!cleared) return;
            queryClient.clear();
            toast.info(
              i18n._(
                msg`Password sign-in enabled. Please sign in to continue.`,
              ),
            );
            await router.invalidate();
            void router.navigate({ to: '/login', replace: true });
          })
          .catch((signOutError: unknown) => {
            toast.error(
              errorMessage(
                signOutError,
                i18n._(
                  msg`Password sign-in is on, but this session could not be signed out.`,
                ),
              ),
            );
          });
      }
    },
    onError: (mutationError, change) => {
      // Back to what the shell holds, keeping later toggles' optimistic values.
      const confirmed = confirmedDesktopSettings.current;
      if (confirmed) applyConfirmedSettings(confirmed, change);
      toast.error(
        errorMessage(
          mutationError,
          i18n._(msg`Failed to update desktop setting`),
        ),
      );
    },
  });

  const backendSettingsMutation = useMutation({
    scope: DESKTOP_SETTINGS_WRITE_SCOPE,
    mutationFn: writeDesktopSettings,
    onSuccess: (updated) => applyConfirmedSettings(updated),
  });

  const skipVersionMutation = useMutation({
    scope: DESKTOP_SETTINGS_WRITE_SCOPE,
    mutationFn: skipDesktopUpdateVersion,
    onSuccess: (updated) => applyConfirmedSettings(updated),
  });

  const autostartMutation = useMutation({
    mutationFn: (enabled: boolean) => setDesktopAutostartEnabled(enabled),
    onMutate: async (enabled) => {
      await queryClient.cancelQueries({ queryKey: DESKTOP_SETTINGS_QUERY_KEY });
      const previous = queryClient.getQueryData<DesktopSettingsSnapshot>(
        DESKTOP_SETTINGS_QUERY_KEY,
      );
      const previousValue = previous?.autostartEnabled;

      if (previous) {
        queryClient.setQueryData<DesktopSettingsSnapshot>(
          DESKTOP_SETTINGS_QUERY_KEY,
          {
            ...previous,
            autostartEnabled: enabled,
          },
        );
      }

      return { previousValue };
    },
    onSuccess: (enabled) => {
      queryClient.setQueryData<DesktopSettingsSnapshot>(
        DESKTOP_SETTINGS_QUERY_KEY,
        (prev) => (prev ? { ...prev, autostartEnabled: enabled } : prev),
      );
    },
    onError: (mutationError, _enabled, context) => {
      if (context && context.previousValue !== undefined) {
        const previousValue = context.previousValue;
        queryClient.setQueryData<DesktopSettingsSnapshot>(
          DESKTOP_SETTINGS_QUERY_KEY,
          (prev) =>
            prev ? { ...prev, autostartEnabled: previousValue } : prev,
        );
      }
      toast.error(
        errorMessage(
          mutationError,
          i18n._(msg`Failed to update launch at login setting`),
        ),
      );
    },
  });

  const handleToggleSetting = (
    key: DesktopBooleanSettingKey,
    checked: boolean,
  ) => {
    settingsMutation.mutate({ [key]: checked });
  };

  // Setting the password signs this session out (the backend revokes every
  // session of the account), and the `requireLogin` write's success path then
  // sends the user to /login. A failed write keeps the toggle off and shows
  // its own toast.
  const handleSetPasswordAndRequireLogin = async (password: string) => {
    try {
      await setDesktopPassword(password);
    } catch (setError) {
      toast.error(
        errorMessage(setError, i18n._(msg`Failed to set the password`)),
      );
      return false;
    }
    queryClient.setQueryData<DesktopSettingsSnapshot>(
      DESKTOP_SETTINGS_QUERY_KEY,
      (prev) => (prev ? { ...prev, passwordSetupRequired: false } : prev),
    );
    settingsMutation.mutate({ requireLogin: true });
    return true;
  };

  const handleSkipVersion = async (version: string | null) => {
    await skipVersionMutation.mutateAsync(version);
  };

  // Leaving a remote server signs out of it first. Local switches need no
  // sign-out: the stored session is tagged with its backend and is dropped on
  // the next launch, even if a recording-exit prompt cancels this restart.
  const restartIntoSavedBackend = async () => {
    if (snapshot?.activeBackendMode === 'remote') {
      await endDesktopBackendSession();
      queryClient.clear();
    }
    await restartDesktopApp();
  };

  const handleApplyBackendSettings = async (
    nextMode: DesktopBackendMode,
    nextRemoteUrl: string | null,
  ) => {
    await backendSettingsMutation.mutateAsync({
      backendMode: nextMode,
      remoteUrl: nextRemoteUrl,
    });
    await restartIntoSavedBackend();
  };

  const handleOpenFolder = async (folder: FolderActionKey) => {
    setOpeningFolder(folder);
    try {
      if (folder === 'recordings') {
        await openDesktopRecordingsFolder();
      } else {
        await openDesktopSystemFolder(folder);
      }
    } catch (openError) {
      toast.error(errorMessage(openError, i18n._(msg`Failed to open folder`)));
    } finally {
      setOpeningFolder((prev) => (prev === folder ? null : prev));
    }
  };

  if (isLoading) {
    return (
      <div className="max-w-3xl space-y-6">
        <SettingsCardSkeleton sections={[4]} />
        <SettingsCardSkeleton sections={[2]} />
        <SettingsCardSkeleton sections={[2]} />
        <SettingsCardSkeleton sections={[3]} />
        <SettingsCardSkeleton sections={[2]} />
      </div>
    );
  }

  if (isError || !snapshot) {
    return (
      <div className="max-w-3xl">
        <SettingsCard
          title={<Trans>Desktop</Trans>}
          description={<Trans>Tray, startup &amp; window behavior</Trans>}
          icon={Monitor}
        >
          <div className="flex flex-col items-start justify-between gap-4 rounded-xl border border-destructive/30 bg-destructive/5 p-4 sm:flex-row sm:items-center">
            <div className="flex items-start gap-3">
              <AlertCircle
                className="mt-0.5 h-5 w-5 shrink-0 text-destructive"
                aria-hidden="true"
              />
              <div className="space-y-1">
                <p className="text-sm font-medium text-destructive">
                  <Trans>Failed to load desktop settings</Trans>
                </p>
                <p className="text-xs text-muted-foreground">
                  {errorMessage(
                    error,
                    i18n._(msg`Unable to communicate with the desktop shell.`),
                  )}
                </p>
              </div>
            </div>
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={() => void refetch()}
              className="shrink-0 gap-2"
            >
              <RefreshCw className="h-3.5 w-3.5" aria-hidden="true" />
              <Trans>Retry</Trans>
            </Button>
          </div>
        </SettingsCard>
      </div>
    );
  }

  return (
    <motion.div
      className="max-w-3xl space-y-6"
      variants={containerVariants}
      initial="hidden"
      animate="visible"
    >
      <motion.div variants={itemVariants}>
        <SettingsCard
          title={<Trans>Startup &amp; Window</Trans>}
          description={
            <Trans>
              Control how Rust-Srec launches and behaves in the system tray.
            </Trans>
          }
          icon={Monitor}
        >
          <div className="space-y-4">
            <SettingToggleRow
              id="desktop-autostart-enabled"
              label={<Trans>Launch at login</Trans>}
              description={
                <Trans>
                  Automatically start Rust-Srec when you sign in to your
                  computer.
                </Trans>
              }
              checked={snapshot.autostartEnabled}
              disabled={autostartMutation.isPending}
              onCheckedChange={(checked) => autostartMutation.mutate(checked)}
            />

            <Separator className="opacity-50" />

            <SettingToggleRow
              id="desktop-start-minimized"
              label={<Trans>Start minimized to tray</Trans>}
              description={
                <Trans>
                  Start quietly in the system tray without opening the main
                  window.
                </Trans>
              }
              checked={snapshot.startMinimized}
              disabled={isSettingPending('startMinimized')}
              onCheckedChange={(checked) =>
                handleToggleSetting('startMinimized', checked)
              }
            />

            <Separator className="opacity-50" />

            <SettingToggleRow
              id="desktop-close-to-tray"
              label={<Trans>Close button hides to tray</Trans>}
              description={
                <Trans>
                  Keep recording in the background when you close the main
                  window instead of quitting.
                </Trans>
              }
              checked={snapshot.closeToTray}
              disabled={isSettingPending('closeToTray')}
              onCheckedChange={(checked) =>
                handleToggleSetting('closeToTray', checked)
              }
            />

            <Separator className="opacity-50" />

            <SettingToggleRow
              id="desktop-minimize-to-tray"
              label={<Trans>Minimize to system tray</Trans>}
              description={
                <Trans>
                  Hide the main window to the system tray instead of the taskbar
                  or Dock when minimized.
                </Trans>
              }
              checked={snapshot.minimizeToTray}
              disabled={isSettingPending('minimizeToTray')}
              onCheckedChange={(checked) =>
                handleToggleSetting('minimizeToTray', checked)
              }
            />
          </div>
        </SettingsCard>
      </motion.div>

      {/* Both settings guard recordings made by the built-in backend. */}
      {snapshot.activeBackendMode === 'local' && (
        <motion.div variants={itemVariants}>
          <SettingsCard
            title={<Trans>Recording Protection</Trans>}
            description={
              <Trans>
                Guard active recordings against sleep and accidental exits.
              </Trans>
            }
            icon={ShieldCheck}
            iconColor="text-emerald-500"
            iconBgColor="bg-emerald-500/10"
          >
            <div className="space-y-4">
              <SettingToggleRow
                id="desktop-prevent-sleep"
                label={<Trans>Prevent system sleep while recording</Trans>}
                description={
                  <Trans>
                    Keep the computer awake while active recordings are running;
                    the display can still turn off.
                  </Trans>
                }
                checked={snapshot.preventSleepWhileRecording}
                disabled={isSettingPending('preventSleepWhileRecording')}
                onCheckedChange={(checked) =>
                  handleToggleSetting('preventSleepWhileRecording', checked)
                }
              />

              <Separator className="opacity-50" />

              <SettingToggleRow
                id="desktop-confirm-exit"
                label={<Trans>Confirm before quitting during recordings</Trans>}
                description={
                  <Trans>
                    Ask for confirmation before quitting when recordings are
                    currently in progress.
                  </Trans>
                }
                checked={snapshot.confirmExitWhileRecording}
                disabled={isSettingPending('confirmExitWhileRecording')}
                onCheckedChange={(checked) =>
                  handleToggleSetting('confirmExitWhileRecording', checked)
                }
              />
            </div>
          </SettingsCard>
        </motion.div>
      )}

      <motion.div variants={itemVariants}>
        <BackendSecurityCard
          snapshot={snapshot}
          isRequireLoginPending={isSettingPending('requireLogin')}
          onToggleRequireLogin={(checked) =>
            handleToggleSetting('requireLogin', checked)
          }
          onSetPasswordAndRequireLogin={handleSetPasswordAndRequireLogin}
          onApplyBackendSettings={handleApplyBackendSettings}
          onRestart={restartIntoSavedBackend}
        />
      </motion.div>

      {snapshot.activeBackendMode === 'local' && (
        <motion.div variants={itemVariants}>
          <ExternalToolsCard />
        </motion.div>
      )}

      <motion.div variants={itemVariants}>
        <SettingsCard
          title={<Trans>Local Folders</Trans>}
          description={
            <Trans>
              Open local directories used for recordings, data, and logs.
            </Trans>
          }
          icon={FolderOpen}
          iconColor="text-amber-500"
          iconBgColor="bg-amber-500/10"
        >
          <div className="space-y-3">
            {snapshot.activeBackendMode === 'local' && (
              <FolderRow
                label={<Trans>Recordings Folder</Trans>}
                path={snapshot.outputDir}
                emptyLabel={<Trans>Not configured</Trans>}
                isOpening={openingFolder === 'recordings'}
                onOpen={() => void handleOpenFolder('recordings')}
              />
            )}
            <FolderRow
              label={<Trans>Data Folder</Trans>}
              path={snapshot.dataDir}
              emptyLabel={<Trans>Unavailable</Trans>}
              isOpening={openingFolder === 'data'}
              onOpen={() => void handleOpenFolder('data')}
            />
            <FolderRow
              label={<Trans>Logs Folder</Trans>}
              path={snapshot.logDir}
              emptyLabel={<Trans>Unavailable</Trans>}
              isOpening={openingFolder === 'logs'}
              onOpen={() => void handleOpenFolder('logs')}
            />
          </div>
        </SettingsCard>
      </motion.div>

      <motion.div variants={itemVariants}>
        <UpdatesCard
          snapshot={snapshot}
          isAutoCheckPending={isSettingPending('checkUpdatesOnStartup')}
          onToggleAutoCheck={(checked) =>
            handleToggleSetting('checkUpdatesOnStartup', checked)
          }
          onSkipVersion={handleSkipVersion}
        />
      </motion.div>
    </motion.div>
  );
}
