import { useQuery } from '@tanstack/react-query';
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

// Resolve the job preset a `preset` step references. `preset` is null while loading, when the
// request failed, and when no preset carries that name, so callers must check `isLoading` and
// `isError` before reporting the preset as missing.
export function usePresetByName(name: string | null, enabled: boolean) {
  const { data, isLoading, isError } = useQuery({
    queryKey: ['job', 'presets', 'detail', name],
    queryFn: () =>
      listJobPresets({ data: { name: name || undefined, limit: 1 } }),
    enabled: enabled && !!name,
  });

  return { preset: findPresetByName(data?.presets, name), isLoading, isError };
}
