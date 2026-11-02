import { z } from 'zod';

const STORAGE_KEY = 'rust-srec:player-danmu';

export const DANMU_AREAS = [0.25, 0.5, 0.75, 1] as const;

/** Seconds a scrolling comment takes to cross the player, slowest first. */
export const DANMU_SPEED_SECS = [12, 10, 8, 6, 4.5] as const;

/** Longest blocklist entry, and most entries per list. */
export const BLOCK_ENTRY_MAX_LENGTH = 64;
export const BLOCK_LIST_MAX_ENTRIES = 200;

const HiddenSchema = z.object({
  scroll: z.boolean().catch(false),
  top: z.boolean().catch(false),
  bottom: z.boolean().catch(false),
  colored: z.boolean().catch(false),
  superchat: z.boolean().catch(false),
});

const blockList = z
  .array(z.string().min(1).max(BLOCK_ENTRY_MAX_LENGTH))
  .max(BLOCK_LIST_MAX_ENTRIES);

/**
 * Viewer preferences for recorded danmu, shared by every player in this
 * browser. Each field falls back to its default on its own, so one bad or
 * missing value never resets the rest.
 */
const DanmuPreferencesSchema = z.object({
  visible: z.boolean().catch(true),
  opacity: z.number().min(0.1).max(1).catch(1),
  area: z
    .number()
    .refine((value) => (DANMU_AREAS as readonly number[]).includes(value))
    .catch(0.75),
  fontScale: z.number().min(0.5).max(2).catch(1),
  /** Index into `DANMU_SPEED_SECS`. */
  speed: z
    .number()
    .int()
    .min(0)
    .max(DANMU_SPEED_SECS.length - 1)
    .catch(2),
  hidden: HiddenSchema.catch(() => HiddenSchema.parse({})),
  merge: z.boolean().catch(false),
  hoverPause: z.boolean().catch(true),
  heatmap: z.boolean().catch(true),
  /** Lower-case; a comment containing any of them is hidden. */
  blockedKeywords: blockList.catch([]),
  /** Exact sender names. */
  blockedUsers: blockList.catch([]),
});

export type DanmuPreferences = z.infer<typeof DanmuPreferencesSchema>;

export function defaultDanmuPreferences(): DanmuPreferences {
  return DanmuPreferencesSchema.parse({});
}

export function readDanmuPreferences(): DanmuPreferences {
  try {
    const stored: unknown = JSON.parse(
      window.localStorage.getItem(STORAGE_KEY) ?? '{}',
    );
    return DanmuPreferencesSchema.parse(
      stored && typeof stored === 'object' ? stored : {},
    );
  } catch {
    // Unreadable storage or malformed JSON.
    return defaultDanmuPreferences();
  }
}

export function storeDanmuPreferences(preferences: DanmuPreferences): void {
  try {
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(preferences));
  } catch {
    // Storage can be unavailable (private mode, quota); the preferences still
    // apply to this player.
  }
}
