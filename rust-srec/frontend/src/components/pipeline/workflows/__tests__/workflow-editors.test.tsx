import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { useState } from 'react';
import type { DagStepDefinition } from '@/api/schemas';
import { PipelineWorkflowEditor } from '../pipeline-workflow-editor';
import { WorkflowEditor } from '../workflow-editor';

vi.mock('@tanstack/react-router', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@tanstack/react-router')>()),
  useNavigate: () => vi.fn(),
}));
vi.mock('../step-library', () => ({ StepLibrary: () => null }));
vi.mock('../flow-editor/workflow-flow-editor', () => ({
  WorkflowFlowEditor: () => null,
}));

const i18n = setupI18n({ locale: 'en', messages: { en: {} } });
const initialSteps: DagStepDefinition[] = [
  { id: 'first', depends_on: [], step: { type: 'workflow', name: 'prepare' } },
  {
    id: 'second',
    depends_on: ['first'],
    step: { type: 'workflow', name: 'archive' },
  },
  {
    id: 'third',
    depends_on: ['first', 'second'],
    step: { type: 'workflow', name: 'notify' },
  },
];

beforeAll(() => {
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;
});

describe.each(['workflow', 'pipeline'] as const)('%s editor', (kind) => {
  it('saves the renamed step and updates every dependent through the real dialog', async () => {
    const onSave = vi.fn();
    function Editor() {
      const [steps, setSteps] = useState(initialSteps);
      return kind === 'pipeline' ? (
        <>
          <PipelineWorkflowEditor steps={steps} onChange={setSteps} />
          <button onClick={() => onSave({ steps })}>Save pipeline</button>
        </>
      ) : (
        <WorkflowEditor
          title="Edit workflow"
          initialData={{
            id: 'workflow',
            name: 'Test',
            description: '',
            dag: { name: 'Test', steps: initialSteps },
            created_at: '',
            updated_at: '',
          }}
          onSubmit={onSave}
        />
      );
    }
    render(
      <I18nProvider i18n={i18n}>
        <QueryClientProvider client={new QueryClient()}>
          <Editor />
        </QueryClientProvider>
      </I18nProvider>,
    );
    fireEvent.click(screen.getAllByTitle('Configure Step')[0]);
    fireEvent.change(await screen.findByLabelText(/Step Identifier/i), {
      target: { value: 'renamed' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Save Changes' }));
    await waitFor(() =>
      expect(screen.queryByRole('dialog')).not.toBeInTheDocument(),
    );
    fireEvent.click(
      screen.getByRole('button', {
        name: kind === 'pipeline' ? 'Save pipeline' : 'Save Workflow',
      }),
    );
    await waitFor(() => expect(onSave).toHaveBeenCalled());
    expect(onSave.mock.calls[0][0].steps).toEqual([
      { ...initialSteps[0], id: 'renamed' },
      { ...initialSteps[1], depends_on: ['renamed'] },
      { ...initialSteps[2], depends_on: ['renamed', 'second'] },
    ]);
  });
});
