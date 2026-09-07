import type { DagStepDefinition, PipelineStep } from '@/api/schemas';

export function createStepId(
  step: PipelineStep,
  steps: DagStepDefinition[],
): string {
  const base = step.type === 'inline' ? step.processor : step.name;
  const existingIds = new Set(steps.map((candidate) => candidate.id));
  let suffix = steps.length;

  while (existingIds.has(`${base}-${suffix}`)) {
    suffix += 1;
  }

  return `${base}-${suffix}`;
}

export function replaceStep(
  steps: DagStepDefinition[],
  index: number,
  step: PipelineStep,
): DagStepDefinition[] {
  return steps.map((candidate, candidateIndex) =>
    candidateIndex === index ? { ...candidate, step } : candidate,
  );
}

export function getStepIdError(
  steps: DagStepDefinition[],
  index: number,
  id: string,
): 'empty' | 'duplicate' | null {
  if (!id.trim()) return 'empty';
  return steps.some(
    (step, candidateIndex) => candidateIndex !== index && step.id === id,
  )
    ? 'duplicate'
    : null;
}

export function updateStep(
  steps: DagStepDefinition[],
  index: number,
  replacement: DagStepDefinition,
): DagStepDefinition[] {
  const original = steps[index];
  if (!original || getStepIdError(steps, index, replacement.id)) return steps;

  return steps.map((candidate, candidateIndex) => {
    const step = candidateIndex === index ? replacement : candidate;
    if (
      original.id === replacement.id ||
      !step.depends_on?.includes(original.id)
    )
      return step;
    return {
      ...step,
      depends_on: step.depends_on.map((id) =>
        id === original.id ? replacement.id : id,
      ),
    };
  });
}

export function removeStep(
  steps: DagStepDefinition[],
  id: string,
): DagStepDefinition[] {
  const removedStep = steps.find((step) => step.id === id);
  if (!removedStep) return steps;

  const predecessors = removedStep.depends_on ?? [];

  return steps
    .filter((step) => step.id !== id)
    .map((step) => {
      if (!step.depends_on?.includes(id)) return step;

      return {
        ...step,
        depends_on: [
          ...new Set([
            ...step.depends_on.filter((dependencyId) => dependencyId !== id),
            ...predecessors,
          ]),
        ],
      };
    });
}
