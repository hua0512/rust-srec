import { act, renderHook } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import type { DagStepDefinition } from '@/api/schemas';
import { useWorkflowSteps } from '../use-workflow-steps';

const steps: DagStepDefinition[] = [
  {
    id: 'download-0',
    step: { type: 'preset', name: 'download' },
    depends_on: [],
  },
  {
    id: 'remux-1',
    step: { type: 'inline', processor: 'remux', config: {} },
    depends_on: ['download-0'],
  },
  {
    id: 'upload-2',
    step: { type: 'workflow', name: 'upload' },
    depends_on: ['remux-1'],
  },
];

function setup(initialSteps: DagStepDefinition[] = steps) {
  const onChange = vi.fn();
  const { result } = renderHook(() =>
    useWorkflowSteps({ steps: initialSteps, onChange }),
  );
  return { onChange, result };
}

describe('useWorkflowSteps', () => {
  it('reads a bare step name as a preset reference', () => {
    const { result } = setup([
      { id: 'legacy', step: 'remux' as never, depends_on: [] },
      ...steps,
    ]);

    expect(result.current.steps[0].step).toEqual({
      type: 'preset',
      name: 'remux',
    });
    expect(result.current.usedStepNames).toEqual([
      'remux',
      'download',
      'remux',
      'upload',
    ]);
  });

  it('keeps the step array identity when nothing needs normalizing', () => {
    const { result } = setup();

    expect(result.current.steps).toBe(steps);
  });

  it('appends a selected step after the last one', () => {
    const { onChange, result } = setup();

    act(() => result.current.selectStep({ type: 'preset', name: 'notify' }));

    expect(onChange).toHaveBeenCalledWith([
      ...steps,
      {
        id: 'notify-3',
        step: { type: 'preset', name: 'notify' },
        depends_on: ['upload-2'],
      },
    ]);
  });

  it('swaps the step being replaced and closes the library', () => {
    const { onChange, result } = setup();

    act(() => result.current.replaceStepAt(1));
    expect(result.current.libraryOpen).toBe(true);
    expect(result.current.selectionMode).toBe('replace');

    act(() => result.current.selectStep({ type: 'preset', name: 'notify' }));

    expect(onChange).toHaveBeenCalledWith([
      steps[0],
      { ...steps[1], step: { type: 'preset', name: 'notify' } },
      steps[2],
    ]);
    expect(result.current.libraryOpen).toBe(false);
    expect(result.current.selectionMode).toBe('add');
  });

  it('forgets the step being replaced when the library is dismissed', () => {
    const { result } = setup();

    act(() => result.current.replaceStepById('remux-1'));
    act(() => result.current.setLibraryOpen(false));

    expect(result.current.selectionMode).toBe('add');
  });

  it('rewires dependencies of a removed step', () => {
    const { onChange, result } = setup();

    act(() => result.current.removeStepAt(1));
    expect(onChange).toHaveBeenCalledWith([
      steps[0],
      { ...steps[2], depends_on: ['download-0'] },
    ]);

    act(() => result.current.removeStepById('download-0'));
    expect(onChange).toHaveBeenLastCalledWith([
      { ...steps[1], depends_on: [] },
      steps[2],
    ]);
  });

  it('saves the step opened in the dialog and closes it', () => {
    const { onChange, result } = setup();

    act(() => result.current.editStepById('remux-1'));
    expect(result.current.editingStep).toBe(steps[1]);
    expect(result.current.editingIndex).toBe(1);

    act(() => result.current.saveEditedStep({ ...steps[1], id: 'renamed' }));

    expect(onChange).toHaveBeenCalledWith([
      steps[0],
      { ...steps[1], id: 'renamed' },
      { ...steps[2], depends_on: ['renamed'] },
    ]);
    expect(result.current.editingStep).toBeNull();
    expect(result.current.editingIndex).toBe(-1);
  });

  it('ignores an edit request for an unknown step', () => {
    const { result } = setup();

    act(() => result.current.editStepById('missing'));

    expect(result.current.editingStep).toBeNull();
  });
});
