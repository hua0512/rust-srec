import { useCallback } from 'react';
import type { Control, FieldPath, FieldValues, Path } from 'react-hook-form';

/**
 * Props every processor config form receives. The host form decides where the
 * processor's config lives (at the root, or under `pathPrefix`), so `control`
 * is typed against the host's untyped values and fields are addressed through
 * {@link useProcessorFieldPath}.
 */
export interface ProcessorConfigFormProps {
  control: Control<FieldValues>;
  pathPrefix?: string;
}

/**
 * Builds field names for a processor config `T` mounted under `pathPrefix`.
 * Keys are checked against `T`, so a misspelled field fails to typecheck
 * instead of silently binding to a value the backend never reads. The
 * returned function is stable while `pathPrefix` is, so it can be an effect
 * dependency.
 */
export function useProcessorFieldPath<T extends FieldValues>(
  pathPrefix?: string,
) {
  return useCallback(
    (key: Path<T>): FieldPath<FieldValues> =>
      pathPrefix ? `${pathPrefix}.${key}` : key,
    [pathPrefix],
  );
}
