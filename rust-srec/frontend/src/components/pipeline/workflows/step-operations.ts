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
