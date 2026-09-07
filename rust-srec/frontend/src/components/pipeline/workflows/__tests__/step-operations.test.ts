import { describe, expect, it } from 'vitest';

import type { DagStepDefinition } from '@/api/schemas';
import {
  createStepId,
  removeStep,
  replaceStep,
  updateStep,
} from '../step-operations';

const steps: DagStepDefinition[] = [
  {
    id: 'download-0',
    step: { type: 'preset', name: 'download' },
    depends_on: [],
  },
  {
    id: 'remux-1',
    step: { type: 'preset', name: 'remux' },
    depends_on: ['download-0'],
  },
  {
    id: 'upload-2',
    step: { type: 'preset', name: 'upload' },
    depends_on: ['remux-1'],
  },
];

describe('pipeline DAG step operations', () => {
  it('renames all dependent references while preserving configuration and dependency edits', () => {
    const branched = [
      ...steps,
      { ...steps[2], id: 'notify', depends_on: ['remux-1', 'download-0'] },
    ];
    const replacement: DagStepDefinition = {
      ...steps[1],
      id: 'converted',
      depends_on: [],
      step: { type: 'inline', processor: 'remux', config: { format: 'mkv' } },
    };
    const result = updateStep(branched, 1, replacement);
    expect(result[1]).toEqual(replacement);
    expect(result[2].depends_on).toEqual(['converted']);
    expect(result[3].depends_on).toEqual(['converted', 'download-0']);
    expect(branched[2].depends_on).toEqual(['remux-1']);
  });

  it.each(['', '   ', 'download-0', 'upload-2'])(
    'rejects invalid ID %j without changing the graph',
    (id) => {
      expect(updateStep(steps, 1, { ...steps[1], id })).toBe(steps);
    },
  );

  it('updates dependency edits when the ID is unchanged', () => {
    const replacement = { ...steps[2], depends_on: ['download-0'] };
    expect(updateStep(steps, 2, replacement)[2]).toEqual(replacement);
  });

  it('reconnects successors to the removed step predecessors', () => {
    const result = removeStep(steps, 'remux-1');

    expect(result).toHaveLength(2);
    expect(result[1]?.depends_on).toEqual(['download-0']);
  });

  it('preserves other dependencies and removes duplicates when bridging', () => {
    const branchedSteps: DagStepDefinition[] = [
      ...steps.slice(0, 2),
      {
        ...steps[2]!,
        depends_on: ['remux-1', 'download-0'],
      },
    ];

    expect(removeStep(branchedSteps, 'remux-1')[1]?.depends_on).toEqual([
      'download-0',
    ]);
  });

  it('replaces only the step content and preserves graph relationships', () => {
    const result = replaceStep(steps, 1, {
      type: 'workflow',
      name: 'archive workflow',
    });

    expect(result[1]).toEqual({
      id: 'remux-1',
      step: { type: 'workflow', name: 'archive workflow' },
      depends_on: ['download-0'],
    });
    expect(result[2]?.depends_on).toEqual(['remux-1']);
  });

  it('creates a unique ID after nodes have been removed', () => {
    const existing = [steps[0]!, steps[2]!];

    expect(createStepId({ type: 'preset', name: 'upload' }, existing)).toBe(
      'upload-3',
    );
  });
});
