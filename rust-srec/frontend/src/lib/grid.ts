// Card grids size their columns from the width they are given, not from viewport
// breakpoints: the app sidebar, and the settings nav on config pages, take a share of the
// viewport that a breakpoint cannot see, so a breakpoint grid packs in columns too narrow for
// a card. `min(100%, …)` lets the single column of a phone-width page shrink below the minimum.

/** Cards with a title, a few fields and actions: streamers, workflows, channels. */
export const CARD_GRID =
  'grid grid-cols-[repeat(auto-fill,minmax(min(100%,18rem),1fr))]';

/** Smaller cards: platforms, templates, sessions, media outputs. */
export const COMPACT_CARD_GRID =
  'grid grid-cols-[repeat(auto-fill,minmax(min(100%,15rem),1fr))]';

/**
 * A row of four stat tiles: one row of four when the container is wide enough, otherwise two
 * by two, never three and an orphan. Needs a `@container` ancestor that spans the row.
 */
export const STAT_GRID = 'grid grid-cols-1 @sm:grid-cols-2 @3xl:grid-cols-4';
