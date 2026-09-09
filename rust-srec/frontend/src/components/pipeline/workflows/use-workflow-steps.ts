import { useCallback, useEffect, useMemo, useState } from 'react';
import type { DagStepDefinition, PipelineStep } from '@/api/schemas';
import {
  createStepId,
  removeStep,
  replaceStep,
  updateStep,
} from './step-operations';

interface UseWorkflowStepsOptions {
  steps: DagStepDefinition[];
  onChange: (steps: DagStepDefinition[]) => void;
}

export interface WorkflowStepsController {
  /** The steps handed in by the caller, in render order. */
  steps: DagStepDefinition[];
  /** Preset, workflow and processor names the step library marks as already used. */
  usedStepNames: string[];
  libraryOpen: boolean;
  /** `replace` while the library is picking a replacement for an existing step. */
  selectionMode: 'add' | 'replace';
  setLibraryOpen: (open: boolean) => void;
  /** Step currently open in the configuration dialog. */
  editingStep: DagStepDefinition | null;
  editingIndex: number;
  editStep: (index: number) => void;
  editStepById: (id: string) => void;
  closeEditor: () => void;
  saveEditedStep: (step: DagStepDefinition) => void;
  replaceStepById: (id: string) => void;
  replaceStepAt: (index: number) => void;
  selectStep: (step: PipelineStep) => void;
  updateStepAt: (index: number, step: DagStepDefinition) => void;
  removeStepAt: (index: number) => void;
  removeStepById: (id: string) => void;
  reorder: (steps: DagStepDefinition[]) => void;
}

function stepName(step: PipelineStep): string {
  return step.type === 'inline' ? step.processor : step.name;
}

/**
 * Owns the editing state shared by the workflow editors: which step the configuration dialog is
 * on, whether the step library is open and which step it is replacing. The steps themselves stay
 * with the caller, which decides where they are stored.
 */
export function useWorkflowSteps({
  steps,
  onChange,
}: UseWorkflowStepsOptions): WorkflowStepsController {
  const [editingIndex, setEditingIndex] = useState<number | null>(null);
  const [libraryOpen, setLibraryOpen] = useState(false);
  const [replacingStepId, setReplacingStepId] = useState<string | null>(null);

  const usedStepNames = useMemo(
    () => steps.map((step) => stepName(step.step)),
    [steps],
  );

  const addStep = useCallback(
    (step: PipelineStep) => {
      const newStep: DagStepDefinition = {
        id: createStepId(step, steps),
        step,
        depends_on: steps.length > 0 ? [steps[steps.length - 1].id] : [],
      };
      onChange([...steps, newStep]);
    },
    [onChange, steps],
  );

  const selectStep = useCallback(
    (step: PipelineStep) => {
      if (replacingStepId === null) {
        addStep(step);
        return;
      }

      const replacementIndex = steps.findIndex(
        (candidate) => candidate.id === replacingStepId,
      );
      if (replacementIndex === -1) return;

      onChange(replaceStep(steps, replacementIndex, step));
      setLibraryOpen(false);
      setReplacingStepId(null);
    },
    [addStep, onChange, replacingStepId, steps],
  );

  const handleLibraryOpenChange = useCallback((open: boolean) => {
    setLibraryOpen(open);
    if (!open) setReplacingStepId(null);
  }, []);

  const replaceStepById = useCallback((id: string) => {
    setReplacingStepId(id);
    setLibraryOpen(true);
  }, []);

  const replaceStepAt = useCallback(
    (index: number) => {
      const step = steps[index];
      if (step) replaceStepById(step.id);
    },
    [replaceStepById, steps],
  );

  const updateStepAt = useCallback(
    (index: number, step: DagStepDefinition) => {
      onChange(updateStep(steps, index, step));
    },
    [onChange, steps],
  );

  const removeStepAt = useCallback(
    (index: number) => {
      const step = steps[index];
      if (step) onChange(removeStep(steps, step.id));
    },
    [onChange, steps],
  );

  const removeStepById = useCallback(
    (id: string) => {
      onChange(removeStep(steps, id));
    },
    [onChange, steps],
  );

  const editStepById = useCallback(
    (id: string) => {
      const index = steps.findIndex((step) => step.id === id);
      if (index !== -1) setEditingIndex(index);
    },
    [steps],
  );

  const closeEditor = useCallback(() => setEditingIndex(null), []);

  // A step can disappear while its dialog is open. Dropping the index as well keeps a later
  // addition at the same position from reopening the dialog on an unrelated step.
  useEffect(() => {
    if (editingIndex !== null && !steps[editingIndex]) setEditingIndex(null);
  }, [editingIndex, steps]);

  const saveEditedStep = useCallback(
    (step: DagStepDefinition) => {
      if (editingIndex === null) return;
      updateStepAt(editingIndex, step);
      setEditingIndex(null);
    },
    [editingIndex, updateStepAt],
  );

  return {
    steps,
    usedStepNames,
    libraryOpen,
    selectionMode: replacingStepId === null ? 'add' : 'replace',
    setLibraryOpen: handleLibraryOpenChange,
    editingStep: editingIndex !== null ? (steps[editingIndex] ?? null) : null,
    editingIndex: editingIndex ?? -1,
    editStep: setEditingIndex,
    editStepById,
    closeEditor,
    saveEditedStep,
    replaceStepById,
    replaceStepAt,
    selectStep,
    updateStepAt,
    removeStepAt,
    removeStepById,
    reorder: onChange,
  };
}
