import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { fireEvent, render, screen } from '@testing-library/react';
import type { PropsWithChildren } from 'react';
import { useForm } from 'react-hook-form';

import { PresetConfigForm } from '../editor/preset-config-form';
import type { PresetFormValues } from '../preset-editor';

vi.mock('motion/react', () => ({
  motion: {
    div: ({ children }: PropsWithChildren) => <div>{children}</div>,
  },
}));

vi.mock('../processors/processor-config-manager', () => ({
  ProcessorConfigManager: () => null,
}));

const i18n = setupI18n({ locale: 'en', messages: { en: {} } });

let currentConfig: unknown;

function Harness({ processor }: { processor: string }) {
  const form = useForm<PresetFormValues>({
    defaultValues: {
      id: '',
      name: '',
      description: '',
      category: '',
      processor,
      config: {},
    },
  });
  currentConfig = form.watch('config');

  return (
    <>
      <button
        type="button"
        onClick={() =>
          form.setValue('config', { width: 64 }, { shouldDirty: true })
        }
      >
        edit
      </button>
      <PresetConfigForm form={form} currentProcessor={processor} />
    </>
  );
}

function renderForm(processor: string) {
  return render(
    <I18nProvider i18n={i18n}>
      <Harness processor={processor} />
    </I18nProvider>,
  );
}

describe('PresetConfigForm', () => {
  it('resets an untouched config to the schema defaults without asking', () => {
    renderForm('thumbnail');

    fireEvent.click(screen.getByRole('button', { name: 'Reset to defaults' }));

    expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
    expect(currentConfig).toMatchObject({
      timestamp_secs: 10,
      width: 320,
      quality: 2,
    });
  });

  it('asks before replacing an edited config', () => {
    renderForm('thumbnail');
    fireEvent.click(screen.getByRole('button', { name: 'edit' }));

    fireEvent.click(screen.getByRole('button', { name: 'Reset to defaults' }));

    expect(screen.getByRole('alertdialog')).toBeInTheDocument();
    expect(currentConfig).toEqual({ width: 64 });

    fireEvent.click(screen.getByRole('button', { name: 'Replace' }));

    expect(currentConfig).toMatchObject({ width: 320 });
  });

  it('keeps an edited config when the replacement is cancelled', () => {
    renderForm('thumbnail');
    fireEvent.click(screen.getByRole('button', { name: 'edit' }));
    fireEvent.click(screen.getByRole('button', { name: 'Reset to defaults' }));

    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));

    expect(currentConfig).toEqual({ width: 64 });
  });

  it('layers the H.264 recipe over the remux defaults', () => {
    renderForm('remux');

    fireEvent.click(screen.getByRole('button', { name: 'H.264' }));

    expect(currentConfig).toMatchObject({
      video_codec: 'h264',
      audio_codec: 'aac',
      crf: 23,
      faststart: true,
      overwrite: true,
    });
  });

  it('offers the H.264 recipe only for remux', () => {
    renderForm('thumbnail');

    expect(
      screen.queryByRole('button', { name: 'H.264' }),
    ).not.toBeInTheDocument();
  });

  it('offers no reset for an unknown processor', () => {
    renderForm('not-a-processor');

    expect(
      screen.queryByRole('button', { name: 'Reset to defaults' }),
    ).not.toBeInTheDocument();
  });
});
