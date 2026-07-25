import { msg } from '@lingui/core/macro';
import type { MessageDescriptor } from '@lingui/core';

export const PRIORITY_LOW = 2;
export const PRIORITY_NORMAL = 5;
export const PRIORITY_HIGH = 8;
export const PRIORITY_CRITICAL = 10;

export type PriorityLevel =
  | typeof PRIORITY_LOW
  | typeof PRIORITY_NORMAL
  | typeof PRIORITY_HIGH
  | typeof PRIORITY_CRITICAL;

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
const PRIORITY_OPTIONS = [
  { value: String(PRIORITY_LOW), label: LOW },
  { value: String(PRIORITY_NORMAL), label: NORMAL },
  { value: String(PRIORITY_HIGH), label: HIGH },
  { value: String(PRIORITY_CRITICAL), label: CRITICAL },
] as const;

export function priorityOptions() {
  return PRIORITY_OPTIONS;
}
