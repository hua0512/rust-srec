import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen } from '@testing-library/react';
import type { DagStepDefinition } from '@/api/schemas';
import { useWorkflowSteps } from '../use-workflow-steps';
import { WorkflowStructurePanel } from '../workflow-structure-panel';

vi.mock('../step-library', () => ({ StepLibrary: () => null }));
vi.mock('../flow-editor/workflow-flow-editor', () => ({
  WorkflowFlowEditor: () => <div>graph view</div>,
}));

const i18n = setupI18n({ locale: 'en', messages: { en: {} } });
const steps: DagStepDefinition[] = [
  { id: 'first', depends_on: [], step: { type: 'workflow', name: 'prepare' } },
];

function Panel({ variant }: { variant: 'standalone' | 'embedded' }) {
  const controller = useWorkflowSteps({ steps, onChange: vi.fn() });
  return <WorkflowStructurePanel controller={controller} variant={variant} />;
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
      render(
        <I18nProvider i18n={i18n}>
          <QueryClientProvider client={new QueryClient()}>
            <Panel variant={variant} />
          </QueryClientProvider>
        </I18nProvider>,
      );

      expect(screen.getByText('Pipeline Structure')).toBeInTheDocument();
      expect(screen.getByText('prepare')).toBeInTheDocument();
      expect(screen.queryByText('graph view')).not.toBeInTheDocument();

      fireEvent.mouseDown(screen.getByRole('tab', { name: 'Graph' }));

      expect(screen.getByText('graph view')).toBeInTheDocument();
      expect(screen.queryByText('prepare')).not.toBeInTheDocument();
    });
  },
);
