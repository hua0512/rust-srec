import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { fireEvent, render, screen } from '@testing-library/react';
import { useForm, type Path, type UseFormReturn } from 'react-hook-form';

import type { DagStepDefinition } from '@/api/schemas';

vi.mock('@/components/pipeline/workflows/pipeline-workflow-editor', () => ({
  PipelineWorkflowEditor: ({
    steps,
    onChange,
  }: {
    steps: DagStepDefinition[];
    onChange: (steps: DagStepDefinition[]) => void;
  }) => (
    <div>
      <span data-testid="steps">{steps.map((s) => s.id).join(',')}</span>
      <button type="button" onClick={() => onChange([])}>
        clear
      </button>
    </div>
  ),
}));

import { PipelineTabsSection } from '../pipeline-tabs-section';

type Values = {
  pipeline: unknown;
  paired_segment_pipeline: unknown;
  session_complete_pipeline: unknown;
};

type PipelineNames = {
  perSegment: Path<Values>;
  paired?: Path<Values>;
  session?: Path<Values>;
};

const INITIAL = {
  name: 'pipeline',
  steps: [{ id: 's1', step: { type: 'inline', processor: 'remux' } }],
};

function renderSection({
  names,
  dagNames,
}: {
  names: PipelineNames;
  dagNames?: { perSegment?: string; paired?: string; session?: string };
}) {
  const i18n = setupI18n({ locale: 'en', messages: { en: {} } });
  let form!: UseFormReturn<Values>;

  function Harness() {
    form = useForm<Values>({ defaultValues: { pipeline: INITIAL } });
    return (
      <PipelineTabsSection form={form} names={names} dagNames={dagNames} />
    );
  }

  render(
    <I18nProvider i18n={i18n}>
      <Harness />
    </I18nProvider>,
  );

  return { value: () => form.getValues('pipeline') };
}

const ALL_NAMES: PipelineNames = {
  perSegment: 'pipeline',
  paired: 'paired_segment_pipeline',
  session: 'session_complete_pipeline',
};

describe('PipelineTabsSection', () => {
  it('offers all three pipelines and loads the selected editor', async () => {
    renderSection({ names: ALL_NAMES });

    expect(screen.getByRole('tab', { name: /Per-segment/ })).toBeDefined();
    expect(screen.getByRole('tab', { name: /Paired/ })).toBeDefined();
    expect(screen.getByRole('tab', { name: /Session/ })).toBeDefined();
    expect((await screen.findByTestId('steps')).textContent).toBe('s1');
  });

  it('explains a pipeline the entity has no field for', async () => {
    renderSection({ names: { perSegment: 'pipeline' } });

    fireEvent.mouseDown(screen.getByRole('tab', { name: /Paired/ }));

    expect(
      await screen.findByText(
        'Paired pipeline is not supported for this entity.',
      ),
    ).toBeDefined();
  });

  // An override field left empty means "inherit from the parent config".
  it('clears an override field when its last step is removed', async () => {
    const section = renderSection({ names: ALL_NAMES });
    await screen.findByTestId('steps');

    fireEvent.click(screen.getByText('clear'));

    expect(section.value()).toBeNull();
  });

  // The global settings request cannot tell a `null` field from an omitted one, so the named
  // pipelines have to clear to a DAG with no steps or the stored pipeline survives the save.
  it('writes an empty DAG when the pipelines are named', async () => {
    const section = renderSection({
      names: ALL_NAMES,
      dagNames: {
        perSegment: 'global_pipeline',
        paired: 'global_paired_pipeline',
        session: 'global_session_pipeline',
      },
    });
    await screen.findByTestId('steps');

    fireEvent.click(screen.getByText('clear'));

    expect(section.value()).toEqual({ name: 'global_pipeline', steps: [] });
  });
});
