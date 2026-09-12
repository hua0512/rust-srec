import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import type { PropsWithChildren } from 'react';

import type { DagStepDefinition, JobPreset } from '@/api/schemas';
import { StepConfigDialog } from '@/components/pipeline/workflows/step-config-dialog';
import { PresetEditor } from '../preset-editor';

vi.mock('@tanstack/react-router', () => ({
  Link: ({ children }: PropsWithChildren) => <a href="#">{children}</a>,
}));

// Exercise the editors and their actual registry schemas without any backend
// connection or process execution.
vi.mock('@/server/functions/job', () => ({
  listJobPresets: vi.fn(async () => ({ presets: [], total: 0 })),
}));

const i18n = setupI18n({ locale: 'en', messages: { en: {} } });
type Editor = 'preset' | 'workflow';

function renderEditor(editor: Editor, config: Record<string, unknown>) {
  const onSave = vi.fn();
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const renderContent = (nextConfig: Record<string, unknown>) => {
    const preset: JobPreset = {
      id: 'execute-test',
      name: 'Execute example',
      processor: 'execute',
      config: nextConfig,
      created_at: '2026-01-01T00:00:00Z',
      updated_at: '2026-01-01T00:00:00Z',
    };
    const step: DagStepDefinition = {
      id: 'execute-test',
      depends_on: [],
      step: { type: 'inline', processor: 'execute', config: nextConfig },
    };
    return (
      <I18nProvider i18n={i18n}>
        <QueryClientProvider client={client}>
          {editor === 'preset' ? (
            <PresetEditor
              initialData={preset}
              title="Edit preset"
              onSubmit={(values) => onSave(values.config)}
            />
          ) : (
            <StepConfigDialog
              open
              onOpenChange={() => {}}
              dagStep={step}
              allSteps={[step]}
              currentStepIndex={0}
              onSave={(values) => {
                if (values.step.type === 'inline') onSave(values.step.config);
              }}
            />
          )}
        </QueryClientProvider>
      </I18nProvider>
    );
  };
  const result = render(renderContent(config));
  return {
    onSave,
    reload: (nextConfig: Record<string, unknown>) =>
      result.rerender(renderContent(nextConfig)),
  };
}

async function selectMode(name: 'Shell command' | 'Program and arguments') {
  fireEvent.keyDown(screen.getByRole('combobox', { name: 'Execution mode' }), {
    key: 'ArrowDown',
  });
  fireEvent.click(await screen.findByRole('option', { name }));
}

function makePresetDirty(editor: Editor) {
  if (editor === 'preset') {
    fireEvent.change(screen.getByRole('textbox', { name: 'Name' }), {
      target: { value: 'Edited execute example' },
    });
  }
}

beforeAll(() => {
  HTMLElement.prototype.scrollIntoView = vi.fn();
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;
});

describe.each<Editor>(['preset', 'workflow'])('Execute %s editor', (editor) => {
  it('loads and saves existing program arguments and null scan settings unchanged', async () => {
    const config = {
      program: 'ffmpeg',
      args: ['', '  ', 'a b', '"quoted"', 'first\nsecond', '{inputs_json}'],
      scan_output_dir: null,
      scan_extension: null,
    };
    const { onSave } = renderEditor(editor, config);

    expect(await screen.findByRole('textbox', { name: 'Program' })).toHaveValue(
      'ffmpeg',
    );
    expect(
      screen.queryByRole('textbox', { name: 'Command' }),
    ).not.toBeInTheDocument();
    for (const [index, value] of config.args.entries()) {
      expect(
        screen.getByRole('textbox', { name: `Argument ${index + 1}` }),
      ).toHaveValue(value);
    }
    makePresetDirty(editor);
    fireEvent.click(screen.getByRole('button', { name: 'Save Changes' }));
    await waitFor(() => expect(onSave).toHaveBeenCalledWith(config));
  });

  it('keeps legacy shell commands editable without conversion', async () => {
    const config = {
      command: 'ffmpeg -i "{input}" -c copy "{output}"',
      scan_output_dir: '/output',
      scan_extension: 'mkv',
    };
    const { onSave } = renderEditor(editor, config);

    expect(await screen.findByRole('textbox', { name: 'Command' })).toHaveValue(
      config.command,
    );
    expect(
      screen.queryByRole('textbox', { name: 'Program' }),
    ).not.toBeInTheDocument();
    makePresetDirty(editor);
    fireEvent.click(screen.getByRole('button', { name: 'Save Changes' }));
    await waitFor(() => expect(onSave).toHaveBeenCalledWith(config));
  });

  it('switches both ways without saving inactive fields or losing scanning settings', async () => {
    const scanning = { scan_output_dir: '/output', scan_extension: 'mp4' };
    const { onSave } = renderEditor(editor, {
      command: 'ffmpeg -version',
      ...scanning,
    });
    await selectMode('Program and arguments');
    fireEvent.change(screen.getByRole('textbox', { name: 'Program' }), {
      target: { value: 'ffprobe' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Add argument' }));
    fireEvent.change(screen.getByRole('textbox', { name: 'Argument 1' }), {
      target: { value: '{input}' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Save Changes' }));
    await waitFor(() =>
      expect(onSave).toHaveBeenLastCalledWith({
        program: 'ffprobe',
        args: ['{input}'],
        ...scanning,
      }),
    );

    await selectMode('Shell command');
    expect(screen.getByRole('textbox', { name: 'Command' })).toHaveValue('');
    fireEvent.change(screen.getByRole('textbox', { name: 'Command' }), {
      target: { value: 'ffmpeg -version' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Save Changes' }));
    await waitFor(() =>
      expect(onSave).toHaveBeenLastCalledWith({
        command: 'ffmpeg -version',
        ...scanning,
      }),
    );
  });

  it('adds and removes argument boxes without trimming or splitting their contents', async () => {
    const { onSave } = renderEditor(editor, {
      program: 'ffmpeg',
      args: ['remove', 'keep'],
    });
    fireEvent.click(screen.getByRole('button', { name: 'Remove argument 1' }));
    fireEvent.click(screen.getByRole('button', { name: 'Add argument' }));
    fireEvent.change(screen.getByRole('textbox', { name: 'Argument 2' }), {
      target: { value: '  "one argument"\nnext line  ' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Add argument' }));
    fireEvent.click(screen.getByRole('button', { name: 'Save Changes' }));
    await waitFor(() =>
      expect(onSave).toHaveBeenCalledWith({
        program: 'ffmpeg',
        args: ['keep', '  "one argument"\nnext line  ', ''],
      }),
    );
  });

  it('places a non-string argument error on its own box and saves after correction', async () => {
    const { onSave } = renderEditor(editor, {
      program: 'ffmpeg',
      args: ['-i', 42],
    });
    makePresetDirty(editor);
    fireEvent.click(screen.getByRole('button', { name: 'Save Changes' }));
    const invalid = screen.getByRole('textbox', { name: 'Argument 2' });
    await waitFor(() =>
      expect(invalid).toHaveAttribute('aria-invalid', 'true'),
    );
    expect(invalid).toHaveAccessibleDescription(/expected string/i);
    expect(screen.getByRole('textbox', { name: 'Argument 1' })).toHaveAttribute(
      'aria-invalid',
      'false',
    );
    expect(onSave).not.toHaveBeenCalled();

    fireEvent.change(invalid, { target: { value: '{input}' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save Changes' }));
    await waitFor(() =>
      expect(onSave).toHaveBeenCalledWith({
        program: 'ffmpeg',
        args: ['-i', '{input}'],
      }),
    );
  });

  it('removes the last argument without restoring its loaded default', async () => {
    const { onSave } = renderEditor(editor, {
      program: 'ffmpeg',
      args: ['{input}'],
    });
    fireEvent.click(screen.getByRole('button', { name: 'Remove argument 1' }));
    expect(
      screen.queryByRole('textbox', { name: 'Argument 1' }),
    ).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Save Changes' }));
    await waitFor(() =>
      expect(onSave).toHaveBeenCalledWith({ program: 'ffmpeg', args: [] }),
    );
  });

  it('requires a nonblank program and clears its errors when switching to shell', async () => {
    const { onSave } = renderEditor(editor, { program: 'ffmpeg' });
    fireEvent.change(screen.getByRole('textbox', { name: 'Program' }), {
      target: { value: '  ' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Save Changes' }));
    expect(await screen.findByText('Program is required')).toBeInTheDocument();
    expect(onSave).not.toHaveBeenCalled();
    await selectMode('Shell command');
    expect(screen.queryByText('Program is required')).not.toBeInTheDocument();
    fireEvent.change(screen.getByRole('textbox', { name: 'Command' }), {
      target: { value: 'ffmpeg -version' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Save Changes' }));
    await waitFor(() =>
      expect(onSave).toHaveBeenCalledWith({ command: 'ffmpeg -version' }),
    );
  });

  it('follows loaded form values when another configuration is opened', async () => {
    const { reload } = renderEditor(editor, {
      program: 'ffmpeg',
      args: ['{input}'],
    });
    expect(await screen.findByRole('textbox', { name: 'Program' })).toHaveValue(
      'ffmpeg',
    );
    reload({ command: 'ffmpeg -version' });
    expect(await screen.findByRole('textbox', { name: 'Command' })).toHaveValue(
      'ffmpeg -version',
    );
    expect(
      screen.queryByRole('textbox', { name: 'Argument 1' }),
    ).not.toBeInTheDocument();
    reload({ program: 'ffprobe', args: [''] });
    expect(await screen.findByRole('textbox', { name: 'Program' })).toHaveValue(
      'ffprobe',
    );
    expect(screen.getByRole('textbox', { name: 'Argument 1' })).toHaveValue('');
  });
});
