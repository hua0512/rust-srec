import { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import type Artplayer from 'artplayer';
import type { DanmuTrack } from '@/components/danmu/danmu-track';
import type { TrackResponse } from '@/components/danmu/danmu-parser.worker';
import { DanmuHeatmap } from './danmu-heatmap';
import { DanmuOverlay } from './danmu-overlay';
import {
  readDanmuPreferences,
  storeDanmuPreferences,
  type DanmuPreferences,
} from './danmu-preferences';
import { DanmuSettingsPanel } from './danmu-settings-panel';

const LAYER = 'danmu';
/**
 * Below this player width the control bar has no room for a separate toggle;
 * the settings panel's own switch covers it.
 */
const NARROW_PLAYER_PX = 480;
const TOGGLE = 'danmu';
const SETTINGS = 'danmu-settings';

const SHOW_DANMU = msg`Show danmu`;
const HIDE_DANMU = msg`Hide danmu`;
const DANMU_SETTINGS = msg`Danmu settings`;

// Lucide's message-square-text, message-square-off and sliders-horizontal.
const ICON_ATTRS =
  'xmlns="http://www.w3.org/2000/svg" width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"';
const ICON_ON = `<svg ${ICON_ATTRS}><path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/><path d="M13 8H7"/><path d="M17 12H7"/></svg>`;
const ICON_OFF = `<svg ${ICON_ATTRS}><path d="M21 15V5a2 2 0 0 0-2-2H9"/><path d="m2 2 20 20"/><path d="M3.6 3.6c-.4.3-.6.8-.6 1.4v16l4-4h10"/></svg>`;
const ICON_SETTINGS = `<svg ${ICON_ATTRS}><path d="M21 4h-7"/><path d="M10 4H3"/><path d="M21 12h-9"/><path d="M8 12H3"/><path d="M21 20h-5"/><path d="M12 20H3"/><path d="M14 2v4"/><path d="M8 10v4"/><path d="M16 18v4"/></svg>`;

/**
 * Loads the danmu XML at `url` into an overlay track, parsed off the main
 * thread. Yields `null` while loading, and also when the file is missing or
 * unreadable: the video then simply plays without danmu.
 */
export function useDanmuTrack(url: string | undefined): DanmuTrack | null {
  const [loaded, setLoaded] = useState<{
    url: string;
    track: DanmuTrack;
  } | null>(null);

  useEffect(() => {
    if (!url) return;

    const worker = new Worker(
      new URL('../danmu/danmu-parser.worker.ts', import.meta.url),
      { type: 'module' },
    );
    worker.addEventListener('message', (event: MessageEvent<TrackResponse>) => {
      if (event.data.type === 'track') {
        setLoaded({ url, track: event.data.track });
      }
      worker.terminate();
    });
    worker.addEventListener('error', () => worker.terminate());
    worker.postMessage({ type: 'track', url });
    return () => worker.terminate();
  }, [url]);

  return loaded && loaded.url === url ? loaded.track : null;
}

interface Mounted {
  overlay: DanmuOverlay;
  heatmap: DanmuHeatmap | null;
  panelHost: HTMLDivElement;
  settingsControl: HTMLElement | undefined;
}

/**
 * Danmu for a recording: the overlay, an activity strip above the progress
 * bar, a show/hide button and a settings panel in the control bar. The panel
 * renders inside the player so it stays usable in fullscreen. Preferences are
 * remembered for every player in this browser.
 */
export function PlayerDanmu({
  player,
  url,
}: {
  player: Artplayer | null;
  url: string | undefined;
}) {
  const { i18n } = useLingui();
  const track = useDanmuTrack(url);
  const [preferences, setPreferences] =
    useState<DanmuPreferences>(readDanmuPreferences);
  const [mounted, setMounted] = useState<Mounted | null>(null);
  const [panelOpen, setPanelOpen] = useState(false);
  const [narrow, setNarrow] = useState(false);
  const preferencesRef = useRef(preferences);
  preferencesRef.current = preferences;

  useEffect(() => {
    if (!player || !track || track.times.length === 0) return;

    const layer = player.layers.add({ name: LAYER, html: '' });
    if (!layer) return;
    const overlay = new DanmuOverlay(layer, player.video, track);
    overlay.configure(preferencesRef.current);

    const progressBar = player.template.$player.querySelector<HTMLElement>(
      '.art-control-progress-inner',
    );
    const heatmap = progressBar
      ? new DanmuHeatmap(progressBar, player.video, track.times)
      : null;

    const panelHost = document.createElement('div');
    // Spans the player so the panel can size to it. The bottom inset keeps
    // the control bar clear, shrinking to 8px in short players (under about
    // 360px) where the panel needs that height more.
    Object.assign(panelHost.style, {
      position: 'absolute',
      inset: '8px 8px clamp(8px, calc(100% - 300px), 62px)',
      display: 'flex',
      alignItems: 'flex-end',
      justifyContent: 'flex-end',
      pointerEvents: 'none',
      zIndex: '90',
    });
    player.template.$player.append(panelHost);

    player.controls.add({
      name: TOGGLE,
      position: 'right',
      index: 10,
      html: ICON_ON,
      click: () =>
        setPreferences((current) => ({
          ...current,
          visible: !current.visible,
        })),
    });
    const settingsControl = player.controls.add({
      name: SETTINGS,
      position: 'right',
      index: 11,
      html: ICON_SETTINGS,
      tooltip: i18n._(DANMU_SETTINGS),
      click: () => setPanelOpen((open) => !open),
    });

    const $player = player.template.$player;
    const sizeObserver =
      typeof ResizeObserver === 'undefined'
        ? null
        : new ResizeObserver(() =>
            setNarrow($player.clientWidth < NARROW_PLAYER_PX),
          );
    sizeObserver?.observe($player);

    setMounted({ overlay, heatmap, panelHost, settingsControl });
    return () => {
      sizeObserver?.disconnect();
      setMounted(null);
      setPanelOpen(false);
      overlay.destroy();
      heatmap?.destroy();
      panelHost.remove();
      if (!player.isDestroy) {
        player.controls.remove(TOGGLE);
        player.controls.remove(SETTINGS);
        player.layers.remove(LAYER);
      }
    };
  }, [player, track, i18n]);

  useEffect(() => {
    if (!mounted || !player) return;
    storeDanmuPreferences(preferences);
    mounted.overlay.configure(preferences);
    mounted.heatmap?.setVisible(preferences.heatmap);
    player.controls.update({
      name: TOGGLE,
      html: preferences.visible ? ICON_ON : ICON_OFF,
      tooltip: i18n._(preferences.visible ? HIDE_DANMU : SHOW_DANMU),
      style: { display: narrow ? 'none' : '' },
    });
  }, [preferences, mounted, player, i18n, narrow]);

  useEffect(() => {
    if (!panelOpen || !mounted) return;
    const close = (event: Event) => {
      const target = event.target as Node | null;
      if (
        target &&
        (mounted.panelHost.contains(target) ||
          mounted.settingsControl?.contains(target))
      )
        return;
      setPanelOpen(false);
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setPanelOpen(false);
    };
    document.addEventListener('pointerdown', close);
    document.addEventListener('keydown', escape);
    return () => {
      document.removeEventListener('pointerdown', close);
      document.removeEventListener('keydown', escape);
    };
  }, [panelOpen, mounted]);

  if (!mounted || !panelOpen) return null;
  return createPortal(
    <div
      className="dark pointer-events-auto w-80 max-w-full max-h-full overflow-y-auto rounded-lg border border-white/10 bg-black/85 p-4 text-foreground shadow-xl backdrop-blur"
      role="dialog"
      aria-label={i18n._(DANMU_SETTINGS)}
    >
      <DanmuSettingsPanel preferences={preferences} onChange={setPreferences} />
    </div>,
    mounted.panelHost,
  );
}
