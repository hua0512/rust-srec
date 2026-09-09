import type {
  FieldValues,
  Path,
  PathValue,
  UseFormReturn,
  UseFormSetValue,
} from 'react-hook-form';

/**
 * Builds the form path for a config field that may live under a prefix.
 *
 * The same field groups are rendered against several forms and at several
 * depths — a global config at the root, a template override under
 * `platform_overrides.<platform>`, a streamer override under
 * `streamer_specific_config` — and a prefix that is only known at runtime
 * cannot be resolved to a member of `Path<T>` by the compiler, so the join is
 * asserted here instead of at every call site.
 */
export function configPath<TFieldValues extends FieldValues>(
  prefix: string | undefined,
  key: string,
): Path<TFieldValues> {
  return (prefix ? `${prefix}.${key}` : key) as Path<TFieldValues>;
}

/**
 * Writes a config field whose value shape is decided at runtime.
 *
 * Fields such as the stream selection config and the pipeline DAG hold either
 * an object or its serialized form depending on the entity being edited, and
 * `PathValue` cannot resolve to that union for a path the component only
 * receives as a prop, so the value is asserted here rather than at each write.
 */
export function setConfigValue<TFieldValues extends FieldValues>(
  form: UseFormReturn<TFieldValues>,
  name: Path<TFieldValues>,
  value: unknown,
  options?: Parameters<UseFormSetValue<TFieldValues>>[2],
): void {
  form.setValue(
    name,
    value as PathValue<TFieldValues, Path<TFieldValues>>,
    options,
  );
}
