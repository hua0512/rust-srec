import { act, fireEvent, render, screen } from '@testing-library/react';
import { useForm, type UseFormReturn } from 'react-hook-form';

import type { DagStepDefinition } from '@/api/schemas';

const { editorRendered, stepsEmitted } = vi.hoisted(() => ({
  editorRendered: vi.fn(),
  stepsEmitted: vi.fn(),
}));

vi.mock('@/components/pipeline/workflows/pipeline-workflow-editor', () => ({
  PipelineWorkflowEditor: ({
    steps,
    onChange,
  }: {
    steps: DagStepDefinition[];
    onChange: (steps: DagStepDefinition[]) => void;
  }) => {
    editorRendered(steps);
    return (
      <div>
        <span data-testid="steps">
          {steps.map((step) => step.id).join(',')}
        </span>
        <button
          type="button"
          onClick={() => {
            const next = [...steps, step(`s${steps.length + 1}`)];
            stepsEmitted(next);
            onChange(next);
          }}
        >
          add
        </button>
        <button type="button" onClick={() => onChange([])}>
          clear
        </button>
      </div>
    );
  },
}));

import { PipelineConfigAdapter } from '../pipeline-config-adapter';

function step(id: string): DagStepDefinition {
  return { id, step: { type: 'inline', processor: 'remux' } };
}

type Values = { pipeline: unknown };

function renderAdapter(initial: unknown, mode: 'json' | 'object' = 'object') {
  let form!: UseFormReturn<Values>;

  function Harness() {
    form = useForm<Values>({ defaultValues: { pipeline: initial } });
    return <PipelineConfigAdapter form={form} name="pipeline" mode={mode} />;
  }

  render(<Harness />);

  return {
    steps: () => screen.getByTestId('steps').textContent,
    value: () => form.getValues('pipeline'),
    setValue: (next: unknown) =>
      act(() => {
        form.setValue('pipeline', next);
      }),
    add: () => fireEvent.click(screen.getByText('add')),
    clear: () => fireEvent.click(screen.getByText('clear')),
  };
}

function renderGlobalAdapter(initial: unknown) {
  let form!: UseFormReturn<Values>;

  function Harness() {
    form = useForm<Values>({ defaultValues: { pipeline: initial } });
    return (
      <PipelineConfigAdapter
        form={form}
        name="pipeline"
        dagName="global_pipeline"
        emptyValue="dag"
      />
    );
  }

  render(<Harness />);

  return {
    steps: () => screen.getByTestId('steps').textContent,
    value: () => form.getValues('pipeline'),
    add: () => fireEvent.click(screen.getByText('add')),
    clear: () => fireEvent.click(screen.getByText('clear')),
  };
}

beforeEach(() => {
  editorRendered.mockClear();
  stepsEmitted.mockClear();
});

describe('PipelineConfigAdapter', () => {
  it('shows the steps held by the form field', () => {
    const adapter = renderAdapter({ name: 'pipeline', steps: [step('s1')] });

    expect(adapter.steps()).toBe('s1');
  });

  it('picks up a value written to the field elsewhere', () => {
    const adapter = renderAdapter(null);

    adapter.setValue({ name: 'pipeline', steps: [step('s1'), step('s2')] });

    expect(adapter.steps()).toBe('s1,s2');
  });

  it('drops the steps when the field is cleared elsewhere', () => {
    const adapter = renderAdapter({ name: 'pipeline', steps: [step('s1')] });

    adapter.setValue(null);

    expect(adapter.steps()).toBe('');
  });

  // The editor's own edit arrives back through the field it wrote to, so it has to settle in a
  // single extra render instead of the two sides rewriting each other.
  it('writes an edit back once and settles', () => {
    const adapter = renderAdapter({ name: 'pipeline', steps: [step('s1')] });
    const rendersBeforeEdit = editorRendered.mock.calls.length;

    adapter.add();

    expect(adapter.value()).toEqual({
      name: 'pipeline',
      steps: [step('s1'), step('s2')],
    });
    expect(adapter.steps()).toBe('s1,s2');
    expect(editorRendered.mock.calls.length).toBe(rendersBeforeEdit + 1);
  });

  // A JSON field parses back into fresh objects, so the editor has to be handed the array it
  // emitted rather than an equal copy of it.
  it('keeps the step identities it emitted for a JSON field', () => {
    const adapter = renderAdapter(
      JSON.stringify({ name: 'pipeline', steps: [step('s1')] }),
      'json',
    );

    adapter.add();

    expect(editorRendered.mock.calls.at(-1)?.[0]).toBe(
      stepsEmitted.mock.calls.at(-1)?.[0],
    );
    expect(JSON.parse(adapter.value() as string)).toEqual({
      name: 'pipeline',
      steps: [step('s1'), step('s2')],
    });
  });

  // An override field left empty means "inherit from the parent config".
  it('clears the field when the last step is removed', () => {
    const adapter = renderAdapter({ name: 'pipeline', steps: [step('s1')] });

    adapter.clear();

    expect(adapter.value()).toBeNull();
    expect(adapter.steps()).toBe('');
  });

  it('stores the DAG under the name the caller asked for', () => {
    const adapter = renderGlobalAdapter(null);

    adapter.add();

    expect(adapter.value()).toEqual({
      name: 'global_pipeline',
      steps: [step('s1')],
    });
  });

  // The global settings request cannot tell a `null` field from an omitted one, so clearing has
  // to be expressed as a DAG with no steps or the stored pipeline survives the save.
  it('writes an empty DAG when asked to keep one', () => {
    const adapter = renderGlobalAdapter({
      name: 'global_pipeline',
      steps: [step('s1')],
    });

    adapter.clear();

    expect(adapter.value()).toEqual({ name: 'global_pipeline', steps: [] });
    expect(adapter.steps()).toBe('');
  });
});
