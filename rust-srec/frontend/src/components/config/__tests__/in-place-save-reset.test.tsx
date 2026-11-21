import { fireEvent, screen, waitFor } from '@testing-library/react';
import type { z } from 'zod';

import { queryKeys } from '@/api/query-keys';
import {
  EngineConfigSchema,
  PlatformConfigSchema,
  TemplateSchema,
} from '@/api/schemas';
import { EngineEditor } from '@/components/config/engines/engine-editor';
import { PlatformEditor } from '@/components/config/platforms/platform-editor';
import { TemplateEditor } from '@/components/config/templates/template-editor';
import { newQueryClient, renderWithProviders } from '@/test/render';

const updateEngineMock = vi.fn();

vi.mock('@tanstack/react-router', async () => ({
  Link: (await import('@/test/router')).MockLink,
}));

vi.mock('@/hooks/use-go-back', () => ({
  useGoBack: () => vi.fn(),
}));

vi.mock('@/components/shared/unsaved-changes-guard', () => ({
  UnsavedChangesGuard: ({ when }: { when: boolean }) => (
    <div data-testid="unsaved-guard" data-when={String(when)} />
  ),
}));

vi.mock('@/server/functions', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@/server/functions')>();
  return {
    ...actual,
    listEngines: vi.fn(async () => []),
    updateEngine: (...args: unknown[]) => updateEngineMock(...args),
  };
});

function isGuardActive() {
  return screen.getByTestId('unsaved-guard').getAttribute('data-when');
}

describe('config editors reset dirty state after saving in place', () => {
  beforeEach(() => {
    updateEngineMock.mockReset();
  });

  it('clears the unsaved-changes guard on PlatformEditor even when the saved config matches the loaded one', async () => {
    const platform = PlatformConfigSchema.parse({
      id: 'platform-twitch',
      name: 'twitch',
      fetch_delay_ms: 5000,
    });
    const client = newQueryClient();
    client.setQueryData(queryKeys.engines.list(), []);

    renderWithProviders(
      <PlatformEditor
        platform={platform}
        onSubmit={(_values, onSaved) => {
          // Simulate a save whose response is unchanged (so the `platform` prop
          // reference never changes under React Query structural sharing).
          onSaved?.(platform);
        }}
        isUpdating={false}
      />,
      { client },
    );

    expect(isGuardActive()).toBe('false');

    const fetchDelay = screen.getByLabelText('Fetch Delay') as HTMLInputElement;
    fireEvent.change(fetchDelay, {
      target: { value: '10' },
    });
    expect(isGuardActive()).toBe('true');

    fireEvent.click(screen.getByRole('button', { name: 'Save Changes' }));
    await waitFor(() => {
      expect(isGuardActive()).toBe('false');
      expect(fetchDelay.value).toBe('5');
    });
  });

  it('clears the unsaved-changes guard on TemplateEditor even when the saved template matches the loaded one', async () => {
    const template = TemplateSchema.parse({
      id: 'tpl-1',
      name: 'Default',
      created_at: '2026-01-01T00:00:00Z',
      updated_at: '2026-01-01T00:00:00Z',
    });
    const client = newQueryClient();
    client.setQueryData(queryKeys.engines.list(), []);

    renderWithProviders(
      <TemplateEditor
        template={template}
        mode="edit"
        onSubmit={(_values, onSaved) => {
          onSaved?.(template);
        }}
        isSubmitting={false}
      />,
      { client },
    );

    expect(isGuardActive()).toBe('false');

    const nameInput = screen.getByLabelText(
      'Template Name',
    ) as HTMLInputElement;
    fireEvent.change(nameInput, {
      target: { value: 'Renamed' },
    });
    expect(isGuardActive()).toBe('true');

    fireEvent.click(screen.getByRole('button', { name: 'Save Changes' }));
    await waitFor(() => expect(isGuardActive()).toBe('false'));
    expect(nameInput.value).toBe('Default');
  });

  it('resets EngineEditor from the saved response and refreshes the cached engine detail', async () => {
    const engine: z.infer<typeof EngineConfigSchema> = EngineConfigSchema.parse(
      {
        id: 'eng-1',
        name: 'Primary FFmpeg',
        engine_type: 'FFMPEG',
        config: { binary_path: 'ffmpeg' },
      },
    );
    const saved: z.infer<typeof EngineConfigSchema> = {
      ...engine,
      name: 'Updated FFmpeg',
    };
    updateEngineMock.mockResolvedValueOnce(saved);

    const onSuccess = vi.fn();
    const client = newQueryClient();
    client.setQueryData(queryKeys.engines.detail(engine.id), engine);

    renderWithProviders(
      <EngineEditor engine={engine} onSuccess={onSuccess} />,
      {
        client,
      },
    );

    expect(isGuardActive()).toBe('false');

    const nameInput = screen.getByLabelText('Engine Name') as HTMLInputElement;
    fireEvent.change(nameInput, {
      target: { value: '  Updated FFmpeg  ' },
    });
    expect(isGuardActive()).toBe('true');

    fireEvent.click(screen.getByRole('button', { name: 'Save changes' }));

    await waitFor(() => expect(onSuccess).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(isGuardActive()).toBe('false'));
    expect(nameInput.value).toBe('Updated FFmpeg');
    expect(client.getQueryData(queryKeys.engines.detail(engine.id))).toEqual(
      saved,
    );
  });
});
