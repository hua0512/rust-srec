import { useState } from 'react';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { toast } from 'sonner';
import {
  AlertCircle,
  CheckCircle2,
  Download,
  ExternalLink,
  Loader2,
  RefreshCw,
} from 'lucide-react';

import { SettingToggleRow } from '@/components/config/desktop/setting-toggle-row';
import { SettingsCard } from '@/components/config/settings-card';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Separator } from '@/components/ui/separator';
import {
  checkForDesktopUpdates,
  openDesktopExternalUrl,
  type DesktopSettingsSnapshot,
  type DesktopUpdateInfo,
} from '@/desktop/desktop-settings';
import { errorMessage } from '@/lib/api-error';

interface UpdatesCardProps {
  snapshot: DesktopSettingsSnapshot;
  isAutoCheckPending: boolean;
  onToggleAutoCheck: (checked: boolean) => void;
  onSkipVersion: (version: string | null) => Promise<void>;
}

export function UpdatesCard({
  snapshot,
  isAutoCheckPending,
  onToggleAutoCheck,
  onSkipVersion,
}: UpdatesCardProps) {
  const { i18n } = useLingui();
  const [updateResult, setUpdateResult] = useState<DesktopUpdateInfo | null>(
    null,
  );
  const [checkError, setCheckError] = useState<string | null>(null);
  const [isChecking, setIsChecking] = useState(false);
  const [isOpeningRelease, setIsOpeningRelease] = useState(false);
  const [isUpdatingSkip, setIsUpdatingSkip] = useState(false);

  const handleCheckForUpdates = async () => {
    setIsChecking(true);
    setCheckError(null);
    try {
      const info = await checkForDesktopUpdates();
      setUpdateResult(info);
    } catch (err) {
      setCheckError(
        errorMessage(err, i18n._(msg`Failed to check for updates.`)),
      );
    } finally {
      setIsChecking(false);
    }
  };

  const handleOpenRelease = async (url: string) => {
    setIsOpeningRelease(true);
    try {
      await openDesktopExternalUrl(url);
    } catch (err) {
      toast.error(
        errorMessage(err, i18n._(msg`Failed to open the release page.`)),
      );
    } finally {
      setIsOpeningRelease(false);
    }
  };

  const handleSkipVersion = async (version: string | null) => {
    setIsUpdatingSkip(true);
    try {
      await onSkipVersion(version);
    } catch (err) {
      toast.error(
        errorMessage(
          err,
          i18n._(msg`Failed to update skipped version preference.`),
        ),
      );
    } finally {
      setIsUpdatingSkip(false);
    }
  };

  const latestVersion = updateResult?.latestVersion;
  const skippedVersion = snapshot.skippedUpdateVersion;
  const isLatestSkipped =
    updateResult !== null && skippedVersion === updateResult.latestVersion;

  return (
    <SettingsCard
      title={<Trans>Updates</Trans>}
      description={<Trans>Check for new Rust-Srec desktop app releases.</Trans>}
      icon={Download}
      iconColor="text-violet-500"
      iconBgColor="bg-violet-500/10"
      action={
        <Badge variant="outline" className="font-mono text-xs">
          v{snapshot.appVersion}
        </Badge>
      }
    >
      <div className="space-y-4">
        <SettingToggleRow
          id="desktop-check-updates-on-startup"
          label={<Trans>Check for updates automatically</Trans>}
          description={
            <Trans>
              Check for newer desktop releases automatically when Rust-Srec
              starts.
            </Trans>
          }
          checked={snapshot.checkUpdatesOnStartup}
          disabled={isAutoCheckPending}
          onCheckedChange={onToggleAutoCheck}
        />

        <Separator className="opacity-50" />

        <div className="flex flex-wrap items-center gap-3">
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={isChecking}
            onClick={() => void handleCheckForUpdates()}
            className="gap-2"
          >
            {isChecking ? (
              <Loader2
                className="h-3.5 w-3.5 animate-spin"
                aria-hidden="true"
              />
            ) : (
              <RefreshCw className="h-3.5 w-3.5" aria-hidden="true" />
            )}
            <Trans>Check for updates</Trans>
          </Button>

          {updateResult && !updateResult.updateAvailable && (
            <Badge
              variant="outline"
              className="border-emerald-500/30 bg-emerald-500/10 text-emerald-600 dark:text-emerald-400"
            >
              <CheckCircle2 className="h-3 w-3" aria-hidden="true" />
              <Trans>You are up to date</Trans>
            </Badge>
          )}
        </div>

        {checkError && (
          <div className="flex items-start gap-2 rounded-xl border border-destructive/30 bg-destructive/5 p-3 text-xs text-destructive">
            <AlertCircle
              className="mt-0.5 h-4 w-4 shrink-0"
              aria-hidden="true"
            />
            <span>{checkError}</span>
          </div>
        )}

        {updateResult && updateResult.updateAvailable && (
          <div className="space-y-3 rounded-xl border border-primary/30 bg-primary/5 p-4">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <div className="flex flex-wrap items-center gap-2">
                <span className="text-sm font-semibold text-foreground">
                  <Trans>Rust-Srec v{latestVersion}</Trans>
                </span>
                <Badge>
                  <Trans>Update available</Trans>
                </Badge>
              </div>
            </div>

            {updateResult.notesExcerpt && (
              <p className="whitespace-pre-line text-xs leading-relaxed text-muted-foreground">
                {updateResult.notesExcerpt}
              </p>
            )}

            <div className="flex flex-wrap items-center gap-2">
              <Button
                type="button"
                size="sm"
                disabled={isOpeningRelease}
                onClick={() => void handleOpenRelease(updateResult.releaseUrl)}
                className="gap-2"
              >
                {isOpeningRelease ? (
                  <Loader2
                    className="h-3.5 w-3.5 animate-spin"
                    aria-hidden="true"
                  />
                ) : (
                  <ExternalLink className="h-3.5 w-3.5" aria-hidden="true" />
                )}
                <Trans>Download release</Trans>
              </Button>

              <Button
                type="button"
                variant="outline"
                size="sm"
                disabled={isUpdatingSkip || isLatestSkipped}
                onClick={() =>
                  void handleSkipVersion(updateResult.latestVersion)
                }
              >
                {isLatestSkipped ? (
                  <Trans>Version skipped</Trans>
                ) : (
                  <Trans>Skip this version</Trans>
                )}
              </Button>
            </div>
          </div>
        )}

        {skippedVersion && (
          <div className="flex flex-wrap items-center gap-x-2 gap-y-1 text-xs text-muted-foreground">
            <span>
              <Trans>
                Update notifications are skipped for v{skippedVersion}.
              </Trans>
            </span>
            <Button
              type="button"
              variant="link"
              size="sm"
              disabled={isUpdatingSkip}
              onClick={() => void handleSkipVersion(null)}
              className="h-auto p-0 text-xs"
            >
              <Trans>Clear skipped version</Trans>
            </Button>
          </div>
        )}
      </div>
    </SettingsCard>
  );
}
