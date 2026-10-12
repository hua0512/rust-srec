import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { zodResolver } from '@hookform/resolvers/zod';
import { useForm, type UseFormReturn } from 'react-hook-form';
import { z } from 'zod';

import {
  MesioConfigOverrideSchema,
  MesioConfigSchema,
} from '@/api/schemas/engine';
import { Form } from '@/components/ui/form';
import { EngineOverrideCard } from '../../../templates/tabs/engine-override-card';
import { MesioDashForm } from '../mesio-dash-form';

type Values = {
  config?: Record<string, unknown>;
  engines_override?: Record<string, Record<string, unknown>>;
};

beforeAll(() => {
  HTMLElement.prototype.scrollIntoView = vi.fn();
  HTMLElement.prototype.hasPointerCapture = vi.fn(() => false);
  HTMLElement.prototype.releasePointerCapture = vi.fn();
});

function mount(override = false, config: Record<string, unknown> = {}) {
  let form!: UseFormReturn<Values>;
  function Harness() {
    form = useForm<Values>({
      resolver: zodResolver(
        z.object({ config: MesioConfigSchema.optional() }),
      ) as never,
      defaultValues: override
        ? { engines_override: { mesio: config } }
        : { config: MesioConfigSchema.parse(config) },
    });
    return (
      <Form {...form}>
        {override ? (
          <EngineOverrideCard
            engineId="mesio"
            engineName="Mesio"
            engineType="MESIO"
            onRemove={vi.fn()}
          />
        ) : (
          <MesioDashForm />
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
    form,
    saved: () =>
      JSON.parse(
        JSON.stringify(
          override
            ? MesioConfigOverrideSchema.parse(
                form.getValues('engines_override.mesio'),
              )
            : MesioConfigSchema.parse(form.getValues('config')),
        ),
      ),
  };
}

async function choose(select: string, option: string) {
  fireEvent.keyDown(screen.getByRole('combobox', { name: select }), {
    key: 'Enter',
  });
  fireEvent.click(await screen.findByRole('option', { name: option }));
}

describe('Mesio DASH settings', () => {
  it('saves nothing until a setting is changed', () => {
    const { saved } = mount();
    expect(saved()).not.toHaveProperty('dash');
    expect(
      screen.getByRole('combobox', { name: 'Video Quality' }),
    ).toHaveTextContent('Default');
    expect(screen.getByRole('switch', { name: /Record Video/ })).toBeChecked();
  });

  it('writes the chosen quality policy with its value', async () => {
    const { saved } = mount();
    await choose('Video Quality', 'Highest up to a height');
    const height = screen.getByLabelText('Maximum height (px)');
    fireEvent.change(height, { target: { value: '720' } });
    expect(saved().dash.video_quality).toEqual({
      type: 'max_height',
      height: 720,
    });

    await choose('Video Quality', 'Closest to a bandwidth');
    expect(screen.queryByLabelText('Maximum height (px)')).toBeNull();
    expect(saved().dash.video_quality).toEqual({
      type: 'closest_bandwidth',
      bandwidth: 5000000,
    });

    await choose('Video Quality', 'Default');
    expect(saved()).not.toHaveProperty('dash');
  });

  it('asks for a delay only for a fixed live start', async () => {
    const { saved } = mount();
    fireEvent.click(screen.getByRole('button', { name: 'Live start' }));
    expect(screen.queryByLabelText(/Delay behind the live edge/)).toBeNull();
    await choose('Live Start Position', 'Fixed delay');
    fireEvent.change(screen.getByLabelText(/Delay behind the live edge/), {
      target: { value: '4000' },
    });
    expect(saved().dash.live_start).toEqual({ type: 'delay', delay_ms: 4000 });
    await choose('Live Start Position', 'Live edge');
    expect(saved().dash.live_start).toEqual({ type: 'live_edge' });
  });

  it('leaves a cleared number to the engine default', () => {
    const { saved } = mount(false, { dash: { download_concurrency: 2 } });
    fireEvent.click(screen.getByRole('button', { name: 'Downloads' }));
    const input = screen.getByLabelText('Download Concurrency');
    expect(input).toHaveValue(2);
    fireEvent.change(input, { target: { value: '' } });
    expect(input).toHaveAttribute('placeholder', 'Default: 5');
    expect(saved()).not.toHaveProperty('dash');
  });

  it('refuses to record neither video nor audio', async () => {
    mount();
    fireEvent.click(screen.getByRole('switch', { name: /Record Video/ }));
    fireEvent.click(
      screen.getByRole('switch', { name: /Record Separate Audio/ }),
    );
    expect(
      await screen.findByText('Record at least one of video and audio'),
    ).toBeInTheDocument();
    fireEvent.click(screen.getByRole('switch', { name: /Record Video/ }));
    await waitFor(() =>
      expect(
        screen.queryByText('Record at least one of video and audio'),
      ).toBeNull(),
    );
  });

  it('is available in template overrides, which stay partial', async () => {
    const { saved } = mount(true, { fix_hls: false });
    fireEvent.mouseDown(screen.getByRole('tab', { name: /DASH Tuning/ }), {
      button: 0,
      ctrlKey: false,
    });
    fireEvent.change(await screen.findByLabelText('Preferred Audio Language'), {
      target: { value: 'pt-BR' },
    });
    expect(saved()).toEqual({
      fix_hls: false,
      dash: { audio_language: 'pt-BR' },
    });
  });
});
