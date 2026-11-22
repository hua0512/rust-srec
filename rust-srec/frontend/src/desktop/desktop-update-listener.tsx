import { useEffect, useRef } from 'react';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { toast } from 'sonner';

import {
  listenDesktopUpdateAvailable,
  openDesktopExternalUrl,
  skipDesktopUpdateVersion,
} from '@/desktop/desktop-settings';
import { isDesktopBuild } from '@/utils/desktop';
import { isTauriRuntime } from '@/utils/tauri';

const UPDATE_TOAST_DURATION_MS = 12_000;

export function DesktopUpdateListener() {
  const { i18n } = useLingui();
  const i18nRef = useRef(i18n);
  i18nRef.current = i18n;

  useEffect(() => {
    if (!isDesktopBuild() || !isTauriRuntime()) return;

    let alive = true;
    let unlisten: (() => void) | null = null;

    void listenDesktopUpdateAvailable((info) => {
      if (!alive || !info.updateAvailable) return;

      const currentI18n = i18nRef.current;

      toast.info(
        currentI18n._(msg`Rust-Srec v${info.latestVersion} is available`),
        {
          id: `desktop-update-${info.latestVersion}`,
          duration: UPDATE_TOAST_DURATION_MS,
          description: currentI18n._(
            msg`You are running v${info.currentVersion}.`,
          ),
          action: {
            label: currentI18n._(msg`Download`),
            onClick: () => {
              void openDesktopExternalUrl(info.releaseUrl).catch((error) => {
                console.error('[desktop] failed to open release URL:', error);
                toast.error(
                  currentI18n._(msg`Failed to open the release page.`),
                );
              });
            },
          },
          cancel: {
            label: currentI18n._(msg`Skip`),
            onClick: () => {
              void skipDesktopUpdateVersion(info.latestVersion).catch(
                (error) => {
                  console.error(
                    '[desktop] failed to skip update version:',
                    error,
                  );
                  toast.error(
                    currentI18n._(msg`Failed to skip this update version.`),
                  );
                },
              );
            },
          },
        },
      );
    })
      .then((cleanup) => {
        if (alive) {
          unlisten = cleanup;
        } else {
          cleanup();
        }
      })
      .catch((error) => {
        console.error('[desktop] failed to subscribe to update event:', error);
      });

    return () => {
      alive = false;
      if (unlisten) unlisten();
    };
  }, []);

  return null;
}
