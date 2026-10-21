/**
 * `value` cut to at most `max` characters (Unicode code points), the unit the
 * backend's length limits and Zod's string checks count. Inputs use this
 * instead of the `maxLength` attribute, which counts UTF-16 code units: an
 * emoji or any other character outside the Basic Multilingual Plane takes two,
 * so `maxLength` stops typing at half the limit. Never cuts a character in two.
 */
export function truncateChars(value: string, max: number): string {
  // A string is never shorter in UTF-16 code units than in characters.
  return value.length <= max ? value : Array.from(value).slice(0, max).join('');
}
