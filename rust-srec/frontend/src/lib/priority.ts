export const PRIORITY_LOW = 2;
export const PRIORITY_NORMAL = 5;
export const PRIORITY_HIGH = 8;
export const PRIORITY_CRITICAL = 10;

export type PriorityLevel =
  | typeof PRIORITY_LOW
  | typeof PRIORITY_NORMAL
  | typeof PRIORITY_HIGH
  | typeof PRIORITY_CRITICAL;

export function priorityLabel(value: number): string {
  if (value <= 3) return 'Low';
  if (value <= 6) return 'Normal';
  if (value <= 9) return 'High';
  return 'Critical';
}

const PRIORITY_OPTIONS = [
  { value: String(PRIORITY_LOW), label: 'Low' },
  { value: String(PRIORITY_NORMAL), label: 'Normal' },
  { value: String(PRIORITY_HIGH), label: 'High' },
  { value: String(PRIORITY_CRITICAL), label: 'Critical' },
] as const;

export function priorityOptions() {
  return PRIORITY_OPTIONS;
}
