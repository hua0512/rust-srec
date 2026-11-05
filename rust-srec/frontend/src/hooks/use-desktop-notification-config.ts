import { useEffect, useState } from 'react';
import {
  initDesktopNotificationsBridge,
  readInitialDesktopNotificationsConfig,
  setDesktopNotificationsConfig,
  type DesktopNotificationConfig,
} from '@/desktop/desktop-notifications';

const DEFAULT_CONFIG: DesktopNotificationConfig = {
  enabled: true,
  minPriority: 5,
  // Empty => no desktop notifications until events are selected.
  eventTypes: [],
};

/**
 * The desktop build's native notification settings, kept in step with the
 * desktop shell, and a setter that saves a change to it. `null` outside the
 * desktop build.
 */
export function useDesktopNotificationConfig(isDesktop: boolean) {
  const [config, setConfig] = useState<DesktopNotificationConfig | null>(() =>
    isDesktop
      ? (readInitialDesktopNotificationsConfig() ?? DEFAULT_CONFIG)
      : null,
  );

  useEffect(() => {
    if (!isDesktop) return;
    let alive = true;
    let unlisten: (() => void) | null = null;

    void (async () => {
      const nextUnlisten = await initDesktopNotificationsBridge((cfg) => {
        if (!alive) return;
        setConfig(cfg);
      });

      if (!alive) {
        nextUnlisten();
        return;
      }

      unlisten = nextUnlisten;
    })();

    return () => {
      alive = false;
      if (unlisten) unlisten();
    };
  }, [isDesktop]);

  const updateConfig = (next: DesktopNotificationConfig) => {
    setConfig(next);
    void setDesktopNotificationsConfig(next);
  };

  return [config, updateConfig] as const;
}
