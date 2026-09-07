import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { fireEvent, render, screen } from '@testing-library/react';
import { useForm } from 'react-hook-form';

import type { DagStepDefinition } from '@/api/schemas';

vi.mock('@/components/pipeline/workflows/pipeline-workflow-editor', () => ({
  PipelineWorkflowEditor: ({ steps }: { steps: DagStepDefinition[] }) => (
    <span data-testid="steps">{steps.map((s) => s.id).join(',')}</span>
  ),
}));

import { PipelineTabsSection } from '../pipeline-tabs-section';

type Values = { pipeline: unknown };

function renderSection(names: {
  perSegment: string;
  paired?: string;
  session?: string;
}) {
  const i18n = setupI18n({ locale: 'en', messages: { en: {} } });

  function Harness() {
    const form = useForm<Values>({
      defaultValues: {
        pipeline: {
          name: 'pipeline',
          steps: [{ id: 's1', step: { type: 'inline', processor: 'remux' } }],
        },
      },
    });
    return <PipelineTabsSection form={form} names={names} />;
  }

  render(
    <I18nProvider i18n={i18n}>
      <Harness />
    </I18nProvider>,
  );
}

describe('PipelineTabsSection', () => {
  it('offers all three pipelines and loads the selected editor', async () => {
    renderSection({
      perSegment: 'pipeline',
      paired: 'paired_segment_pipeline',
      session: 'session_complete_pipeline',
    });

    expect(screen.getByRole('tab', { name: /Per-segment/ })).toBeDefined();
    expect(screen.getByRole('tab', { name: /Paired/ })).toBeDefined();
    expect(screen.getByRole('tab', { name: /Session/ })).toBeDefined();
    expect((await screen.findByTestId('steps')).textContent).toBe('s1');
  });

  it('explains a pipeline the entity has no field for', async () => {
    renderSection({ perSegment: 'pipeline' });

    fireEvent.mouseDown(screen.getByRole('tab', { name: /Paired/ }));

    expect(
      await screen.findByText(
        'Paired pipeline is not supported for this entity.',
      ),
    ).toBeDefined();
  });
});
