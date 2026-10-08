import { UserRound } from 'lucide-react';
import { cn } from '@/lib/utils';
import { validityStyle, type Validity } from './account-health';

/** Scripts written without spaces between words, where one character reads as a name. */
const SINGLE_CHARACTER_SCRIPT =
  /\p{Script=Han}|\p{Script=Hiragana}|\p{Script=Katakana}|\p{Script=Hangul}/u;

/** Characters that can start an initial, as opposed to emoji and symbols. */
const LETTER_OR_DIGIT = /^[\p{L}\p{N}]/u;

function graphemes(text: string): string[] {
  if (typeof Intl !== 'undefined' && 'Segmenter' in Intl)
    return Array.from(
      new Intl.Segmenter(undefined, { granularity: 'grapheme' }).segment(text),
      ({ segment }) => segment,
    );
  return Array.from(text);
}

/**
 * Up to two characters standing for an account label: the first letters of
 * its first two words, or the first two letters of a single word. A label
 * starting in Chinese, Japanese or Korean gives its first character, and one
 * starting with an emoji or symbol gives that alone. A blank label gives none.
 */
export function accountInitials(label: string): string {
  const words = label.trim().split(/\s+/).filter(Boolean);
  if (!words.length) return '';
  const first = graphemes(words[0]);
  if (SINGLE_CHARACTER_SCRIPT.test(first[0]) || !LETTER_OR_DIGIT.test(first[0]))
    return first[0];
  const second = words[1] && graphemes(words[1])[0];
  const initials =
    second &&
    LETTER_OR_DIGIT.test(second) &&
    !SINGLE_CHARACTER_SCRIPT.test(second)
      ? first[0] + second
      : first.slice(0, 2).join('');
  return initials.toLocaleUpperCase();
}

/** Soft tints that read on light and dark surfaces, away from the health hues. */
const AVATAR_TONES = [
  'bg-sky-500/15 text-sky-700 dark:text-sky-300',
  'bg-violet-500/15 text-violet-700 dark:text-violet-300',
  'bg-indigo-500/15 text-indigo-700 dark:text-indigo-300',
  'bg-teal-500/15 text-teal-700 dark:text-teal-300',
  'bg-fuchsia-500/15 text-fuchsia-700 dark:text-fuchsia-300',
  'bg-orange-500/15 text-orange-700 dark:text-orange-300',
] as const;

/** The tint of an account's avatar, the same for a label on every page. */
export function avatarTone(label: string): string {
  let hash = 0;
  for (const character of label)
    hash = (hash * 31 + (character.codePointAt(0) ?? 0)) >>> 0;
  return AVATAR_TONES[hash % AVATAR_TONES.length];
}

/**
 * An account's initials on a tint of its own, with its health as a dot at the
 * corner. A disabled account is greyed out and its dot hollow. Decorative: the
 * row around it names the account and its state.
 */
export function AccountAvatar({
  label,
  validity,
  enabled,
  className,
}: {
  label: string;
  validity: Validity | undefined;
  enabled: boolean;
  className?: string;
}) {
  const initials = accountInitials(label);
  return (
    <span
      aria-hidden="true"
      className={cn(
        'relative flex size-9 shrink-0 select-none items-center justify-center rounded-full text-xs font-semibold',
        enabled ? avatarTone(label) : 'bg-muted text-muted-foreground/70',
        className,
      )}
    >
      {initials || <UserRound className="size-4" />}
      <span
        className={cn(
          'absolute -right-0.5 -bottom-0.5 size-3 rounded-full ring-2 ring-card',
          enabled
            ? validityStyle(validity).dot
            : 'border-2 border-muted-foreground/50 bg-card',
        )}
      />
    </span>
  );
}
