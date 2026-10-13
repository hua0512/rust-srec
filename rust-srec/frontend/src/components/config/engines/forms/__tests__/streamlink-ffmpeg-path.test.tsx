import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { fireEvent, render, screen } from '@testing-library/react';
import { useForm, type UseFormReturn } from 'react-hook-form';

import {
  StreamlinkConfigOverrideSchema,
  StreamlinkConfigSchema,
} from '@/api/schemas/engine';
import { Form } from '@/components/ui/form';
import { EngineOverrideCard } from '../../../templates/tabs/engine-override-card';
import { StreamlinkForm } from '../streamlink-form';

type Values = {
  config?: Record<string, unknown>;
  engines_override?: Record<string, Record<string, unknown>>;
};

beforeAll(() => {
  HTMLElement.prototype.scrollIntoView = vi.fn();
  HTMLElement.prototype.hasPointerCapture = vi.fn(() => false);
  HTMLElement.prototype.releasePointerCapture = vi.fn();
});

function renderEngine(config: Record<string, unknown>) {
  let form!: UseFormReturn<Values>;
  function Harness() {
    form = useForm<Values>({ defaultValues: { config } });
    return (
      <Form {...form}>
        <StreamlinkForm />
      </Form>
    );
  }
  render(
    <I18nProvider i18n={setupI18n({ locale: 'en', messages: { en: {} } })}>
      <Harness />
    </I18nProvider>,
  );
  return () =>
    JSON.parse(
      JSON.stringify(StreamlinkConfigSchema.parse(form.getValues('config'))),
    );
}

function renderOverride(
  override: Record<string, unknown>,
  engineConfig: Record<string, unknown>,
) {
  let form!: UseFormReturn<Values>;
  function Harness() {
    form = useForm<Values>({
      defaultValues: { engines_override: { 'streamlink-1': override } },
    });
    return (
      <Form {...form}>
        <EngineOverrideCard
          engineId="streamlink-1"
          engineName="Streamlink"
          engineType="STREAMLINK"
          engineConfig={engineConfig}
          onRemove={vi.fn()}
        />
      </Form>
    );
  }
  render(
    <I18nProvider i18n={setupI18n({ locale: 'en', messages: { en: {} } })}>
      <Harness />
    </I18nProvider>,
  );
  return () =>
    JSON.parse(
      JSON.stringify(
        StreamlinkConfigOverrideSchema.parse(
          form.getValues('engines_override.streamlink-1'),
        ),
      ),
    );
}

async function choose(select: string, option: string) {
  fireEvent.keyDown(screen.getByRole('combobox', { name: select }), {
    key: 'Enter',
  });
  fireEvent.click(await screen.findByRole('option', { name: option }));
}

describe('Streamlink FFmpeg path field', () => {
  it('saves an entered path literally and a cleared one as the environment default', () => {
    const saved = renderEngine({ quality: 'best' });
    const input = screen.getByLabelText('FFmpeg Path');
    expect(input).toHaveAttribute(
      'placeholder',
      'Default: FFMPEG_PATH or ffmpeg',
    );
    fireEvent.change(input, {
      target: { value: ' C:\\Media Tools\\ffmpeg.exe ' },
    });
    expect(saved().ffmpeg_path).toBe(' C:\\Media Tools\\ffmpeg.exe ');
    fireEvent.change(input, { target: { value: '' } });
    expect(saved()).not.toHaveProperty('ffmpeg_path');
  });

  it.each([
    ['C:\\Media Tools\\ffmpeg.exe', 'C:\\Media Tools\\ffmpeg.exe'],
    // A null path is unset, which also uses the environment.
    [null, undefined],
    // The backend reads empty text as a path; only the user changes it.
    ['', ''],
    ['   ', '   '],
  ])('keeps a saved %j when other engine settings change', (path, expected) => {
    const saved = renderEngine({ quality: 'best', ffmpeg_path: path });
    fireEvent.change(screen.getByLabelText('Quality'), {
      target: { value: 'worst' },
    });
    expect(saved().quality).toBe('worst');
    expect(saved().ffmpeg_path).toBe(expected);
  });

  it('keeps inherited and environment-default template overrides distinct', async () => {
    const saved = renderOverride({}, { ffmpeg_path: '/opt/ffmpeg' });
    expect(
      screen.getByRole('combobox', { name: 'FFmpeg Path' }),
    ).toHaveTextContent('Use engine setting (/opt/ffmpeg)');
    expect(saved()).toEqual({});

    await choose('FFmpeg Path', 'Default (FFMPEG_PATH or ffmpeg)');
    expect(saved().ffmpeg_path).toBeNull();

    await choose('FFmpeg Path', 'Custom');
    const input = screen.getByRole('textbox', { name: 'FFmpeg Path' });
    expect(input).toHaveValue('/opt/ffmpeg');
    fireEvent.change(input, { target: { value: '/opt/custom/ffmpeg' } });
    expect(saved().ffmpeg_path).toBe('/opt/custom/ffmpeg');

    await choose('FFmpeg Path', 'Use engine setting (/opt/ffmpeg)');
    expect(saved()).toEqual({});
  });

  it('shows an existing environment-default override as the default', () => {
    renderOverride({ ffmpeg_path: null }, { ffmpeg_path: '/opt/ffmpeg' });
    expect(
      screen.getByRole('combobox', { name: 'FFmpeg Path' }),
    ).toHaveTextContent('Default (FFMPEG_PATH or ffmpeg)');
  });
});
