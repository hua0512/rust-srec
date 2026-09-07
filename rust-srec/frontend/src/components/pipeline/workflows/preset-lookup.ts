import { queryOptions, useQueries, useQuery } from '@tanstack/react-query';
import type { DagStepDefinition } from '@/api/schemas';
import { listJobPresets } from '@/server/functions/job';

// Pick the preset whose name equals `name`. Preset names are unique, and the backend resolves a
// `preset` step by exact name, so anything else is a different preset with a different processor.
// `listJobPresets({ name })` already filters server-side; this equality check keeps a row that
// arrived through any other filter (e.g. `search`, which also matches descriptions) from being
// mistaken for the named preset.
export function findPresetByName<T extends { name: string }>(
  presets: readonly T[] | undefined,
  name: string | null | undefined,
): T | null {
  if (!presets || !name) return null;
  return presets.find((p) => p.name === name) ?? null;
}

function presetByNameOptions(name: string | null, enabled: boolean) {
  return queryOptions({
    queryKey: ['job', 'presets', 'detail', name],
    queryFn: () =>
      listJobPresets({ data: { name: name || undefined, limit: 1 } }),
    enabled: enabled && !!name,
  });
}

// Check isLoading/isError before treating a null preset as missing.
export function usePresetByName(name: string | null, enabled: boolean) {
  const { data, isLoading, isError } = useQuery(
    presetByNameOptions(name, enabled),
  );

  return { preset: findPresetByName(data?.presets, name), isLoading, isError };
}

// Resolve only referenced names. All editor surfaces share the dialog's exact-name cache,
// independent of the list endpoint's page limit.
export function useReferencedPresets(
  steps: DagStepDefinition[],
  enabled = true,
) {
  const names = [
    ...new Set(
      steps.flatMap(({ step }) => (step.type === 'preset' ? [step.name] : [])),
    ),
  ];
  return useQueries({
    queries: names.map((name) => presetByNameOptions(name, enabled)),
    combine: (results) => ({
      presets: results.flatMap((result, index) => {
        const preset = findPresetByName(result.data?.presets, names[index]);
        return !result.isError && preset ? [preset] : [];
      }),
      loading: names.filter((_, index) => results[index].isPending),
      failed: names.filter((_, index) => results[index].isError),
      missing: names.filter(
        (name, index) =>
          results[index].isSuccess &&
          !findPresetByName(results[index].data?.presets, name),
      ),
    }),
  });
}
