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

const inlineSteps: DagStepDefinition[] = [
  {
    id: 'run',
    depends_on: [],
    step: { type: 'inline', processor: 'execute', config: { command: 'echo' } },
  },
];

function renderTree(node: React.ReactNode) {
  return render(
    <I18nProvider i18n={i18n}>
      <QueryClientProvider client={new QueryClient()}>
        {node}
      </QueryClientProvider>
    </I18nProvider>,
  );
}

/** Edits the inline step's command, then submits the dialog's own form as Enter would. */
async function editStepAndSubmitTheDialogForm() {
  fireEvent.click(screen.getAllByTitle('Configure Step')[0]);
  fireEvent.change(await screen.findByLabelText('Command'), {
    target: { value: 'echo edited' },
  });
  const dialogForm = document.querySelector('[role="dialog"] form');
  expect(dialogForm).not.toBeNull();
  fireEvent.submit(dialogForm!);
  await waitFor(() =>
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument(),
  );
}

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

describe('step dialog submit', () => {
  it('saves the step without submitting the workflow form around it', async () => {
    const onSubmit = vi.fn();
    renderTree(
      <WorkflowEditor
        title="Edit workflow"
        initialData={{
          id: 'workflow',
          name: 'Test',
          description: '',
          dag: { name: 'Test', steps: inlineSteps },
          created_at: '',
          updated_at: '',
        }}
        onSubmit={onSubmit}
      />,
    );

    await editStepAndSubmitTheDialogForm();

    expect(onSubmit).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole('button', { name: 'Save Workflow' }));

    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(1));
    expect(onSubmit.mock.calls[0][0].steps[0].step.config.command).toBe(
      'echo edited',
    );
  });

  it('saves the step without submitting the settings form around it', async () => {
    const onSubmit = vi.fn((event: React.FormEvent) => event.preventDefault());
    function Settings() {
      const [steps, setSteps] = useState(inlineSteps);
      return (
        <form onSubmit={onSubmit}>
          <PipelineWorkflowEditor steps={steps} onChange={setSteps} />
          <div data-testid="command">
            {(steps[0].step as { config: { command: string } }).config.command}
          </div>
        </form>
      );
    }
    renderTree(<Settings />);

    await editStepAndSubmitTheDialogForm();

    expect(onSubmit).not.toHaveBeenCalled();
    expect(screen.getByTestId('command')).toHaveTextContent('echo edited');
  });
});
