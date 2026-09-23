import { msg } from '@lingui/core/macro';
import type { MessageDescriptor } from '@lingui/core';

export const PRIORITY_LOW = 2;
export const PRIORITY_NORMAL = 5;
export const PRIORITY_HIGH = 8;
export const PRIORITY_CRITICAL = 10;

/**
 * The four bands a numeric priority falls into. Shared by `priorityLabel` and
 * `PRIORITY_OPTIONS` so the word shown on a badge always matches the one in the select.
 *
 * `context` keeps these off the unqualified "Low"/"High"/... used for unrelated scales.
 */
const LOW = msg({ message: 'Low', context: 'priority' });
const NORMAL = msg({ message: 'Normal', context: 'priority' });
const HIGH = msg({ message: 'High', context: 'priority' });
const CRITICAL = msg({ message: 'Critical', context: 'priority' });

/** Resolve with `i18n._`; a plain string here would render untranslated. */
export function priorityLabel(value: number): MessageDescriptor {
  if (value <= 3) return LOW;
  if (value <= 6) return NORMAL;
  if (value <= 9) return HIGH;
  return CRITICAL;
}

/**
 * `value` is the wire value the select writes back; `label` is a descriptor the caller resolves
 * through `i18n._`. Returning a plain string here would leave the option list in English, since
 * `<Trans>{expr}</Trans>` treats its child as an interpolated value rather than a message.
 */
export const PRIORITY_OPTIONS = [
  { value: String(PRIORITY_LOW), label: LOW },
  { value: String(PRIORITY_NORMAL), label: NORMAL },
  { value: String(PRIORITY_HIGH), label: HIGH },
  { value: String(PRIORITY_CRITICAL), label: CRITICAL },
] as const;

/**
 * Notification channels deliver events at or above a threshold, so their select describes each
 * choice as a floor rather than a single band.
 */
export const MIN_PRIORITY_OPTIONS = [
  { value: String(PRIORITY_CRITICAL), label: msg`Critical Only` },
  { value: String(PRIORITY_HIGH), label: msg`High+` },
  { value: String(PRIORITY_NORMAL), label: msg`Normal+` },
  { value: String(PRIORITY_LOW), label: msg`All` },
] as const;

/** Resolve with `i18n._`. */
export function minPriorityLabel(value: number): MessageDescriptor {
  if (value >= 10) return MIN_PRIORITY_OPTIONS[0].label;
  if (value >= 7) return MIN_PRIORITY_OPTIONS[1].label;
  if (value >= 4) return MIN_PRIORITY_OPTIONS[2].label;
  return MIN_PRIORITY_OPTIONS[3].label;
}
