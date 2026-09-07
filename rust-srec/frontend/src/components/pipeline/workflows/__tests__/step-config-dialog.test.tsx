import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from '@testing-library/react';
import { DagStepDefinition } from '@/api/schemas';
import { StepConfigDialog } from '../step-config-dialog';
import { StepsList } from '../steps-list';

// Stands in for `/api/job/presets`: `search` is a substring of name AND description ordered by
// name, `name` is an exact match. `delete_source` mentions "thumbnail" in its description and
// sorts first, so a search-based lookup that keeps the first row resolves to the delete preset.
const PRESETS = [
  ...Array.from({ length: 120 }, (_, index) => ({
    id: `custom-${index}`,
    name: `a-custom-${index}`,
    description: 'Unrelated thumbnail configuration',
    category: 'thumbnail',
    processor: 'thumbnail',
    config: {},
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
  })),
  {
    id: 'preset-default-delete',
    name: 'delete_source',
    description: 'Deletes the files produced by the previous thumbnail step.',
    category: 'cleanup',
    processor: 'delete',
    config: { max_retries: 3 },
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
  },
  {
    id: 'preset-default-thumbnail',
    name: 'thumbnail',
    description: 'Generate a thumbnail',
    category: 'thumbnail',
    processor: 'thumbnail',
    config: { timestamp_secs: 42, width: 640, quality: 3 },
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
  },
];

let available = PRESETS;

const listJobPresets = vi.fn(
  async ({
    data,
  }: {
    data: { name?: string; search?: string; limit?: number };
  }) => {
    let rows = available;
    if (data.name) {
      rows = rows.filter((p) => p.name === data.name);
    } else if (data.search) {
      const needle = data.search.toLowerCase();
      rows = rows.filter(
        (p) =>
          p.name.toLowerCase().includes(needle) ||
          p.description.toLowerCase().includes(needle),
      );
    }
    rows = [...rows]
      .sort((a, b) => a.name.localeCompare(b.name))
      .slice(0, Math.min(data.limit ?? 20, 100));
    return {
      presets: rows,
      categories: [],
      total: rows.length,
      limit: data.limit ?? 20,
      offset: 0,
    };
  },
);

vi.mock('@/server/functions/job', () => ({
  listJobPresets: (args: any) => listJobPresets(args),
}));

const i18n = setupI18n({ locale: 'en', messages: { en: {} } });

function renderDialog(
  step: DagStepDefinition,
  onSave = vi.fn(),
  allSteps = [step],
) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <I18nProvider i18n={i18n}>
      <QueryClientProvider client={client}>
        <StepConfigDialog
          open
          onOpenChange={() => {}}
          dagStep={step}
          onSave={onSave}
          allSteps={allSteps}
          currentStepIndex={allSteps.indexOf(step)}
        />
      </QueryClientProvider>
    </I18nProvider>,
  );
  return { onSave, client };
}

beforeAll(() => {
  // Radix's ScrollArea observes its viewport; jsdom has no ResizeObserver.
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;
});

beforeEach(() => {
  listJobPresets.mockClear();
  available = PRESETS;
});

const transform: DagStepDefinition = {
  id: 'thumb',
  depends_on: [],
  step: { type: 'preset', name: 'thumbnail' },
};
const deletion: DagStepDefinition = {
  id: 'cleanup',
  depends_on: ['thumb'],
  step: { type: 'preset', name: 'delete_source' },
};

describe('workflow preset warnings', () => {
  it('resolves labels and delete warnings beyond the first 100 presets without borrowing inline labels', async () => {
    const inline: DagStepDefinition = {
      id: 'custom',
      depends_on: [],
      step: { type: 'inline', processor: 'thumbnail', config: {} },
    };
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    render(
      <I18nProvider i18n={i18n}>
        <QueryClientProvider client={client}>
          <StepsList
            steps={[transform, deletion, inline]}
            onReorder={() => {}}
          />
        </QueryClientProvider>
      </I18nProvider>,
    );
    await screen.findByTitle(/This Delete step deletes the converted result/i);
    expect(screen.getByText('Inline: thumbnail')).toBeInTheDocument();
    expect(screen.queryByText('a-custom-0')).not.toBeInTheDocument();
    expect(screen.queryByRole('status')).not.toBeInTheDocument();
    expect(listJobPresets).toHaveBeenCalledTimes(2);
    expect(listJobPresets).toHaveBeenCalledWith({
      data: { name: 'thumbnail', limit: 1 },
    });
  });

  it('requires confirmation for a delete preset found beyond the first page', async () => {
    const { onSave } = renderDialog(deletion, vi.fn(), [transform, deletion]);
    await waitFor(() =>
      expect(screen.queryByRole('status')).not.toBeInTheDocument(),
    );
    fireEvent.click(screen.getByRole('button', { name: /Save Changes/i }));
    await screen.findByRole('heading', {
      name: 'Delete the converted result?',
    });
    expect(onSave).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: /Add anyway/i }));
    await waitFor(() => expect(onSave).toHaveBeenCalledWith(deletion));
  });

  it.each(['missing', 'failed', 'loading'])(
    'does not silently approve a delete with a %s dependency preset',
    async (state) => {
      const inlineDelete: DagStepDefinition = {
        ...deletion,
        step: { type: 'inline', processor: 'delete', config: {} },
      };
      if (state === 'missing')
        available = PRESETS.filter((p) => p.name !== 'thumbnail');
      if (state === 'failed')
        listJobPresets.mockRejectedValueOnce(new Error('Unavailable'));
      if (state === 'loading')
        listJobPresets.mockImplementationOnce(() => new Promise(() => {}));
      const { onSave } = renderDialog(inlineDelete, vi.fn(), [
        transform,
        inlineDelete,
      ]);
      const statusText =
        state === 'missing'
          ? /Missing presets:/
          : state === 'failed'
            ? /Could not load presets:/
            : /Loading presets:/;
      await screen.findByText(statusText);
      fireEvent.click(screen.getByRole('button', { name: /Save Changes/i }));
      await screen.findByRole('heading', {
        name: 'Save with incomplete delete checks?',
      });
      expect(onSave).not.toHaveBeenCalled();
    },
  );

  it.each(['', '   ', 'thumb'])(
    'prevents saving invalid step ID %j',
    async (id) => {
      const { onSave } = renderDialog(deletion, vi.fn(), [transform, deletion]);
      fireEvent.mouseDown(screen.getByRole('tab', { name: /Flow/i }), {
        button: 0,
        ctrlKey: false,
      });
      fireEvent.change(await screen.findByLabelText(/Step Identifier/i), {
        target: { value: id },
      });
      expect(
        screen.getByRole('button', { name: /Save Changes/i }),
      ).toBeDisabled();
      expect(
        await screen.findByText(
          id.trim() ? 'Step ID must be unique.' : 'Step ID is required.',
        ),
      ).toBeInTheDocument();
      expect(onSave).not.toHaveBeenCalled();
    },
  );
});

describe('StepConfigDialog preset steps', () => {
  it('detaches a preset step into its own processor', async () => {
    const step: DagStepDefinition = {
      id: 'thumb',
      depends_on: [],
      step: { type: 'preset', name: 'thumbnail' },
    };
    const { onSave } = renderDialog(step);

    const detach = await screen.findByRole('button', {
      name: /Detach & Edit/i,
    });
    await waitFor(() => expect(detach).toBeEnabled());

    fireEvent.click(detach);
    fireEvent.click(screen.getByRole('button', { name: /Save Changes/i }));

    // The written processor is the named preset's, never another preset's.
    await waitFor(() => expect(onSave).toHaveBeenCalled());
    expect(onSave.mock.calls[0][0].step).toMatchObject({
      type: 'inline',
      processor: 'thumbnail',
    });

    // Which it can only be because the preset is looked up by identity, not by substring.
    expect(listJobPresets).toHaveBeenCalledWith({
      data: { name: 'thumbnail', limit: 1 },
    });
  });

  it('saves the detached edits after the preset stops resolving', async () => {
    const step: DagStepDefinition = {
      id: 'thumb',
      depends_on: [],
      step: { type: 'preset', name: 'thumbnail' },
    };
    const { onSave, client } = renderDialog(step);

    const detach = await screen.findByRole('button', {
      name: /Detach & Edit/i,
    });
    await waitFor(() => expect(detach).toBeEnabled());
    fireEvent.click(detach);

    // The preset is deleted elsewhere and the query refetches without it. handleDetach latched
    // the processor, so the edit form stays up and performSave still writes an inline step.
    available = PRESETS.filter((p) => p.name !== 'thumbnail');
    await act(async () => {
      await client.refetchQueries({
        queryKey: ['job', 'presets', 'detail', 'thumbnail'],
      });
      // react-query notifies its observers on a microtask; yield so the re-render lands inside
      // this act() rather than after the click below.
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
    expect(screen.queryByText(/No configuration form available/i)).toBeNull();

    fireEvent.click(screen.getByRole('button', { name: /Save Changes/i }));
    await waitFor(() => expect(onSave).toHaveBeenCalled());
    expect(onSave.mock.calls[0][0].step).toMatchObject({
      type: 'inline',
      processor: 'thumbnail',
    });
  });

  it('keeps a step whose preset does not exist and offers no detach', async () => {
    const step: DagStepDefinition = {
      id: 'gone',
      depends_on: [],
      step: { type: 'preset', name: 'renamed_away' },
    };
    const { onSave } = renderDialog(step);

    await screen.findByText(/No preset named/i);
    expect(
      screen.getByRole('button', { name: /Detach & Edit/i }),
    ).toBeDisabled();

    fireEvent.click(screen.getByRole('button', { name: /Save Changes/i }));
    await waitFor(() => expect(onSave).toHaveBeenCalled());
    expect(onSave.mock.calls[0][0].step).toEqual({
      type: 'preset',
      name: 'renamed_away',
    });
  });
});
