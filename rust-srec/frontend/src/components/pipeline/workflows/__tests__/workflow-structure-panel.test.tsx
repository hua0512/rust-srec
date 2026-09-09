import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen } from '@testing-library/react';
import type { DagStepDefinition } from '@/api/schemas';
import { useWorkflowSteps } from '../use-workflow-steps';
import { WorkflowStructurePanel } from '../workflow-structure-panel';

vi.mock('../flow-editor/workflow-flow-editor', () => ({
  WorkflowFlowEditor: () => <div>graph view</div>,
}));
vi.mock('@/server/functions/job', () => ({
  listJobPresets: async () => ({
    presets: [],
    categories: [],
    total: 0,
    limit: 20,
    offset: 0,
  }),
}));
vi.mock('@/server/functions/pipeline', () => ({
  listPipelinePresets: async () => ({
    presets: [],
    total: 0,
    limit: 20,
    offset: 0,
  }),
}));

const i18n = setupI18n({ locale: 'en', messages: { en: {} } });
const steps: DagStepDefinition[] = [
  { id: 'first', depends_on: [], step: { type: 'workflow', name: 'prepare' } },
];

function Panel({
  variant,
  libraryTrigger,
}: {
  variant: 'standalone' | 'embedded';
  libraryTrigger?: React.ReactNode;
}) {
  const controller = useWorkflowSteps({ steps, onChange: vi.fn() });
  return (
    <WorkflowStructurePanel
      controller={controller}
      variant={variant}
      className="structure-panel"
      libraryTrigger={libraryTrigger}
    />
  );
}

function renderPanel(
  variant: 'standalone' | 'embedded',
  libraryTrigger?: React.ReactNode,
) {
  return render(
    <I18nProvider i18n={i18n}>
      <QueryClientProvider client={new QueryClient()}>
        <Panel variant={variant} libraryTrigger={libraryTrigger} />
      </QueryClientProvider>
    </I18nProvider>,
  );
}

beforeAll(() => {
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;
});

describe.each(['standalone', 'embedded'] as const)(
  '%s workflow structure panel',
  (variant) => {
    it('shows the steps as a list and switches to the graph view', () => {
      renderPanel(variant);

      expect(screen.getByText('Pipeline Structure')).toBeInTheDocument();
      expect(screen.getByText('prepare')).toBeInTheDocument();
      expect(screen.queryByText('graph view')).not.toBeInTheDocument();

      fireEvent.mouseDown(screen.getByRole('tab', { name: 'Graph' }));

      expect(screen.getByText('graph view')).toBeInTheDocument();
      expect(screen.queryByText('prepare')).not.toBeInTheDocument();
    });
  },
);

describe('workflow structure panel step library', () => {
  it('carries the step library next to the view tabs when embedded', () => {
    const { container } = renderPanel('embedded');

    const panel = container.querySelector('.structure-panel');
    const library = screen.getByText('Step Library');
    expect(panel).not.toBeNull();
    expect(panel?.contains(library)).toBe(true);
  });

  it('renders the given trigger instead of the default library card', () => {
    renderPanel('embedded', <button type="button">Add Step</button>);

    expect(
      screen.getByRole('button', { name: 'Add Step' }),
    ).toBeInTheDocument();
    expect(screen.queryByText('Step Library')).not.toBeInTheDocument();
  });

  it('leaves the step library to the page when standalone', () => {
    renderPanel('standalone');

    expect(screen.getByText('Pipeline Structure')).toBeInTheDocument();
    expect(screen.queryByText('Step Library')).not.toBeInTheDocument();
  });
});
