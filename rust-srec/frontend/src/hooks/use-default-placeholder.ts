import { useCallback } from 'react';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';

/**
 * Placeholder for an optional field that falls back to a built-in default.
 *
 * Engine and limit forms are full of fields whose placeholder just names the value used when the
 * field is left blank. Interpolating the value keeps that one translatable string instead of one
 * per field, which is how dozens of `"Default: 30000"` literals ended up untranslated.
 *
 * ```tsx
 * const defaultPlaceholder = useDefaultPlaceholder();
 * <Input placeholder={defaultPlaceholder(30000)} />
 * ```
 */
export function useDefaultPlaceholder() {
  const { i18n } = useLingui();

  return useCallback(
    (value: string | number) => i18n._(msg`Default: ${value}`),
    [i18n],
  );
}
