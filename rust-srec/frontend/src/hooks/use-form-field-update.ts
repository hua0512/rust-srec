import { useCallback } from 'react';
import { UseFormReturn, FieldValues, Path, PathValue } from 'react-hook-form';

interface UseFormFieldUpdateOptions {
  shouldDirty?: boolean;
  shouldTouch?: boolean;
  shouldValidate?: boolean;
}

const DEFAULT_OPTIONS: UseFormFieldUpdateOptions = {
  shouldDirty: true,
  shouldTouch: true,
  shouldValidate: true,
};

/**
 * Hook that provides a memoized function to update form fields with consistent options.
 * Reduces duplication of the "form.setValue with shouldDirty/shouldTouch/shouldValidate" pattern.
 *
 * @example
 * ```tsx
 * const updateField = useFormFieldUpdate(form);
 * // Later...
 * updateField('fieldName', newValue);
 * ```
 */
export function useFormFieldUpdate<TFieldValues extends FieldValues>(
  form: UseFormReturn<TFieldValues>,
  options: UseFormFieldUpdateOptions = DEFAULT_OPTIONS,
) {
  const mergedOptions = { ...DEFAULT_OPTIONS, ...options };

  const updateField = useCallback(
    <TFieldName extends Path<TFieldValues>>(
      name: TFieldName,
      value: PathValue<TFieldValues, TFieldName>,
    ) => {
      form.setValue(name, value, mergedOptions);
    },
    [form, mergedOptions],
  );

  return updateField;
}

/**
 * Hook for managing nested object state synced with a form field.
 * Useful for complex objects like RetryPolicy that need individual field updates.
 *
 * @example
 * ```tsx
 * const [policy, updatePolicy] = useNestedFormState(form, 'retry_policy', DEFAULT_POLICY);
 * // Later...
 * updatePolicy('max_retries', 5);
 * ```
 */
export function useNestedFormState<
  TFieldValues extends FieldValues,
  TFieldName extends Path<TFieldValues>,
  TValue extends Record<string, any>,
>(
  form: UseFormReturn<TFieldValues>,
  name: TFieldName,
  defaultValue: TValue,
  options: UseFormFieldUpdateOptions & { mode?: 'json' | 'object' } = {},
) {
  const { mode = 'object', ...updateOptions } = options;
  const currentVal = form.watch(name);

  // Parse current value
  let parsedValue: TValue;
  if (currentVal) {
    if (mode === 'json' && typeof currentVal === 'string') {
      try {
        parsedValue = { ...defaultValue, ...JSON.parse(currentVal) };
      } catch {
        parsedValue = defaultValue;
      }
    } else if (typeof currentVal === 'object') {
      parsedValue = { ...defaultValue, ...currentVal };
    } else {
      parsedValue = defaultValue;
    }
  } else {
    parsedValue = defaultValue;
  }

  const updateNestedField = useCallback(
    <K extends keyof TValue>(key: K, value: TValue[K]) => {
      const newValue = { ...parsedValue, [key]: value };
      const formValue = mode === 'json' ? JSON.stringify(newValue) : newValue;
      form.setValue(name, formValue as PathValue<TFieldValues, TFieldName>, {
        shouldDirty: updateOptions.shouldDirty ?? true,
        shouldTouch: updateOptions.shouldTouch ?? true,
        shouldValidate: updateOptions.shouldValidate ?? true,
      });
    },
    [form, name, parsedValue, mode, updateOptions],
  );

  return [parsedValue, updateNestedField] as const;
}
