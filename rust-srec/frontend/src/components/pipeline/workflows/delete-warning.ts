import { useQuery } from '@tanstack/react-query';
import { useMemo } from 'react';
import { DagStepDefinition } from '@/api/schemas';
import { listJobPresets } from '@/server/functions/job';

// Processors that produce a NEW primary artifact distinct from their input (e.g. the remux
// family transcodes to a new file). The DAG feeds each step the outputs of the steps it
// depends_on, so a `delete` step depending on one of these receives the produced file and would
// delete the converted result, not the original recording. The transcode family exposes
// `remove_input_on_success` to delete the source in place instead.
export const TRANSFORM_PROCESSORS = new Set([
  'remux',
  'transcode',
  'convert',
  'compression',
  'audio_extract',
  'thumbnail',
  'ass_burnin',
  'danmaku_factory',
  'metadata',
]);

// Build a preset-name -> processor lookup from the job preset list.
export function buildPresetProcessorMap(
  presets: { name: string; processor: string }[] | undefined,
): Map<string, string> {
  const map = new Map<string, string>();
  for (const p of presets ?? []) map.set(p.name, p.processor);
  return map;
}

// Resolve a step to its processor: inline steps carry it directly; preset steps are looked up in
// the name -> processor map; workflow steps expand into their own steps later and are skipped.
export function resolveStepProcessor(
  step: DagStepDefinition | null | undefined,
  presetProcessorByName: Map<string, string>,
): string | null {
  const st = step?.step;
  if (!st) return null;
  if (st.type === 'inline') return st.processor;
  if (st.type === 'preset') return presetProcessorByName.get(st.name) ?? null;
  return null;
}

// For a delete step, the ids of its dependencies that are transform steps (whose outputs are
// converted artifacts, not the source). Empty unless `step` is a delete step with at least one
// such dependency.
export function getTransformDependencyIds(
  step: DagStepDefinition,
  allSteps: DagStepDefinition[],
  presetProcessorByName: Map<string, string>,
): string[] {
  if (resolveStepProcessor(step, presetProcessorByName) !== 'delete') return [];
  return (step.depends_on ?? []).filter((depId) => {
    const depProc = resolveStepProcessor(
      allSteps.find((s) => s.id === depId),
      presetProcessorByName,
    );
    return depProc != null && TRANSFORM_PROCESSORS.has(depProc);
  });
}

// Step ids that are delete steps wired directly after a transform step (a data-loss risk).
export function getDeleteAfterTransformStepIds(
  steps: DagStepDefinition[],
  presetProcessorByName: Map<string, string>,
): Set<string> {
  const ids = new Set<string>();
  for (const step of steps) {
    if (
      getTransformDependencyIds(step, steps, presetProcessorByName).length > 0
    ) {
      ids.add(step.id);
    }
  }
  return ids;
}

// Shared preset name -> processor map. Reuses a single query (deduped by key across the step
// editor dialog and the graph view).
export function usePresetProcessorMap(enabled = true): Map<string, string> {
  const { data } = useQuery({
    queryKey: ['job', 'presets', 'processor-map'],
    queryFn: () => listJobPresets({ data: { limit: 200 } }),
    enabled,
  });
  return useMemo(() => buildPresetProcessorMap(data?.presets), [data]);
}
