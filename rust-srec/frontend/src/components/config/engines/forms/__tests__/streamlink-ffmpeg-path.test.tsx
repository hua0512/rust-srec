import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { fireEvent, render, screen } from '@testing-library/react';
import { useForm, type UseFormReturn } from 'react-hook-form';

import {
  StreamlinkConfigSchema,
  type StreamlinkConfig,
} from '@/api/schemas/engine';
import { Form } from '@/components/ui/form';
import { EngineOverrideCard } from '../../../templates/tabs/engine-override-card';
import { StreamlinkForm } from '../streamlink-form';

type Values = {
  config?: StreamlinkConfig;
  engines_override?: Record<string, Partial<StreamlinkConfig>>;
};

function renderForm(ffmpegPath: string | null | undefined, override = false) {
  let form!: UseFormReturn<Values>;
  const path = override
    ? 'engines_override.streamlink-1.ffmpeg_path'
    : 'config.ffmpeg_path';
  const config =
    ffmpegPath === undefined
      ? { quality: 'best' }
      : { quality: 'best', ffmpeg_path: ffmpegPath };
  function Harness() {
    form = useForm<Values>({
      defaultValues: override
        ? { engines_override: { 'streamlink-1': config } }
        : { config: StreamlinkConfigSchema.parse(config) },
    });
    return (
      <Form {...form}>
        {override ? (
          <EngineOverrideCard
            engineId="streamlink-1"
            engineName="Streamlink"
            engineType="STREAMLINK"
            onRemove={vi.fn()}
          />
        ) : (
          <StreamlinkForm />
        )}
      </Form>
    );
  }
  render(
    <I18nProvider i18n={setupI18n({ locale: 'en', messages: { en: {} } })}>
      <Harness />
    </I18nProvider>,
  );
  return {
    value: () => form.getValues(path),
    serialized: () => JSON.parse(JSON.stringify(form.getValues())),
  };
}

describe('Streamlink FFmpeg path field', () => {
  it.each(['C:\\Media Tools\\ffmpeg.exe', '', '   ', null, undefined])(
    'preserves saved %s when other engine settings change',
    (path) => {
      const { value } = renderForm(path);
      fireEvent.change(screen.getByLabelText('Quality'), {
        target: { value: 'worst' },
      });
      expect(value()).toBe(path);
    },
  );

  it('keeps entered paths literal and clears a base path to environment fallback', () => {
    const { value } = renderForm(null);
    const input = screen.getByLabelText('FFmpeg Path');
    fireEvent.change(input, {
      target: { value: ' C:\\Media Tools\\ffmpeg.exe ' },
    });
    expect(value()).toBe(' C:\\Media Tools\\ffmpeg.exe ');
    fireEvent.change(input, { target: { value: '' } });
    expect(value()).toBeNull();
    expect(input).toHaveAttribute('placeholder', 'FFMPEG_PATH or ffmpeg');
  });

  it('allows an explicitly empty saved executable to be reset', () => {
    const { value } = renderForm('');
    fireEvent.click(
      screen.getByRole('button', { name: 'Use environment default' }),
    );
    expect(value()).toBeNull();
  });

  it('keeps inherited and environment-default template overrides distinct', () => {
    const { value, serialized } = renderForm(undefined, true);
    const input = screen.getByLabelText('FFmpeg Path');
    expect(value()).toBeUndefined();
    expect(serialized().engines_override['streamlink-1']).not.toHaveProperty(
      'ffmpeg_path',
    );
    expect(input).toHaveAttribute('placeholder', 'Inherited from engine');
    fireEvent.click(
      screen.getByRole('button', { name: 'Use environment default' }),
    );
    expect(value()).toBeNull();
    expect(
      serialized().engines_override['streamlink-1'].ffmpeg_path,
    ).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'Use engine setting' }));
    expect(value()).toBeUndefined();
    expect(serialized().engines_override['streamlink-1']).not.toHaveProperty(
      'ffmpeg_path',
    );
  });

  it('preserves an existing null override and clears custom paths back to inheritance', () => {
    const { value } = renderForm(null, true);
    const input = screen.getByLabelText('FFmpeg Path');
    expect(value()).toBeNull();
    expect(input).toHaveAttribute('placeholder', 'FFMPEG_PATH or ffmpeg');
    fireEvent.change(input, { target: { value: '/opt/custom/ffmpeg' } });
    expect(value()).toBe('/opt/custom/ffmpeg');
    fireEvent.change(input, { target: { value: '' } });
    expect(value()).toBeUndefined();
  });
});
