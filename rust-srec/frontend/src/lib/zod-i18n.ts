import type { I18n, MessageDescriptor } from '@lingui/core';
import { msg, plural } from '@lingui/core/macro';
import { z } from 'zod';
import { en, zhCN } from 'zod/locales';

// Zod resolves an issue's message when it validates, not when the schema is built, so schema
// constants shared with server validators can carry translatable messages. The browser binds its
// one Lingui instance here. The server binds only a fixed English instance as the fallback: a
// per-request binding would leak one request's locale into another, and production builds compile
// `msg` descriptors to their id alone, so without a catalog there is no text to show.
let active: I18n | undefined;
let fallback: I18n | undefined;

const zodLocales: Record<string, ReturnType<typeof en>> = {
  en: en(),
  'zh-CN': zhCN(),
};

/**
 * The message for an issue raised inside a refinement, in the active locale. Call it while the
 * schema validates, not while it is built.
 */
export function zodText(descriptor: MessageDescriptor): string | undefined {
  const i18n = active ?? fallback;
  return i18n ? i18n._(descriptor) : descriptor.message;
}

/** Sets the English instance that server validators word their messages with. */
export function setZodFallbackI18n(i18n: I18n) {
  fallback = i18n;
}

/** A schema's error parameter whose message follows the active locale. */
export function zodError(descriptor: MessageDescriptor) {
  return { error: () => zodText(descriptor) };
}

const REQUIRED = msg({ message: 'Required', context: 'validation' });

function sizeMessage(
  i18n: I18n,
  issue: z.core.$ZodRawIssue,
): string | undefined {
  if (issue.code !== 'too_small' && issue.code !== 'too_big') return;
  const small = issue.code === 'too_small';
  const count = Number(small ? issue.minimum : issue.maximum);
  if (!Number.isFinite(count)) return;
  switch (issue.origin) {
    case 'string':
      if (small && count === 1) return i18n._(REQUIRED);
      return small
        ? i18n._(
            msg`Enter at least ${plural(count, { one: '# character', other: '# characters' })}`,
          )
        : i18n._(
            msg`Enter at most ${plural(count, { one: '# character', other: '# characters' })}`,
          );
    case 'array':
    case 'set':
      return small
        ? i18n._(
            msg`Add at least ${plural(count, { one: '# item', other: '# items' })}`,
          )
        : i18n._(
            msg`Add at most ${plural(count, { one: '# item', other: '# items' })}`,
          );
    case 'number':
    case 'int':
    case 'bigint':
      if (small) {
        return issue.inclusive === false
          ? i18n._(msg`Must be greater than ${count}`)
          : i18n._(msg`Must be at least ${count}`);
      }
      return issue.inclusive === false
        ? i18n._(msg`Must be less than ${count}`)
        : i18n._(msg`Must be at most ${count}`);
  }
}

function localeError(
  issue: z.core.$ZodRawIssue,
): ReturnType<z.core.$ZodErrorMap> {
  const i18n = active;
  if (!i18n) return;
  switch (issue.code) {
    case 'invalid_type':
      if (issue.input === undefined || issue.input === null) {
        return i18n._(REQUIRED);
      }
      if (issue.expected === 'number' && Number.isNaN(issue.input)) {
        return i18n._(msg`Enter a number`);
      }
      if (issue.expected === 'int') return i18n._(msg`Enter a whole number`);
      break;
    case 'too_small':
    case 'too_big': {
      const message = sizeMessage(i18n, issue);
      if (message) return message;
      break;
    }
    case 'invalid_format':
      if (issue.format === 'url') return i18n._(msg`Enter a valid URL`);
      if (issue.format === 'email') {
        return i18n._(msg`Enter a valid email address`);
      }
      break;
  }
  return (zodLocales[i18n.locale] ?? zodLocales.en).localeError(issue);
}

/**
 * Translates Zod's messages, its defaults and those declared with {@link zodError} or
 * {@link zodText}, with this Lingui instance. Browser only.
 */
export function bindZodToI18n(i18n: I18n) {
  active = i18n;
  z.config({ localeError });
}
