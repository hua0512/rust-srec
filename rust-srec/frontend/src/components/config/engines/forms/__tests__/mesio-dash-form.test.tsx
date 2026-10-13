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
import { withEngineOverrideChecks } from '../../../templates/engine-override-checks';
import { EngineOverrideCard } from '../../../templates/tabs/engine-override-card';
import { MesioForm } from '../mesio-form';

type Values = {
  config?: Record<string, unknown>;
  engines_override?: Record<string, Record<string, unknown>>;
};

beforeAll(() => {
  HTMLElement.prototype.scrollIntoView = vi.fn();
  HTMLElement.prototype.hasPointerCapture = vi.fn(() => false);
  HTMLElement.prototype.releasePointerCapture = vi.fn();
});

function mount(
  override = false,
  config: Record<string, unknown> = {},
  engineConfig: Record<string, unknown> = {},
) {
  let form!: UseFormReturn<Values>;
  const engine = MesioConfigSchema.parse(engineConfig);
  function Harness() {
    form = useForm<Values>({
      resolver: override
        ? withEngineOverrideChecks<Values>(
            zodResolver(
              z.object({
                engines_override: z
                  .record(z.string(), MesioConfigOverrideSchema)
                  .optional(),
              }),
            ) as never,
            () => [{ id: 'mesio', engine_type: 'MESIO', config: engine }],
            () => NO_TRACK,
          )
        : (zodResolver(
            z.object({ config: MesioConfigSchema.optional() }),
          ) as never),
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
            engineConfig={engine}
            onRemove={vi.fn()}
          />
        ) : (
          <MesioForm />
        )}
      </Form>
    );
  }
  render(
    <I18nProvider i18n={setupI18n({ locale: 'en', messages: { en: {} } })}>
      <Harness />
    </I18nProvider>,
  );
  openDashTab();
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

const NO_TRACK = 'Records neither video nor audio';

function openDashTab() {
  fireEvent.mouseDown(screen.getByRole('tab', { name: /^DASH/ }), {
    button: 0,
    ctrlKey: false,
  });
}

function openSection(name: string) {
  fireEvent.click(screen.getByRole('button', { name: new RegExp(`^${name}`) }));
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
    ).toHaveTextContent('Default (Highest bandwidth)');
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

    await choose('Video Quality', 'Default (Highest bandwidth)');
    expect(saved()).not.toHaveProperty('dash');
  });

  it('asks for a delay only for a fixed live start', async () => {
    const { saved } = mount();
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
    openSection('Segment downloads');
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
    await choose('Preferred Audio Language', 'Custom');
    fireEvent.change(
      screen.getByRole('textbox', { name: 'Preferred Audio Language' }),
      { target: { value: 'pt-BR' } },
    );
    expect(saved()).toEqual({
      fix_hls: false,
      dash: { audio_language: 'pt-BR' },
    });
  });
});

describe('Mesio DASH template overrides', () => {
  it('shows what unset settings inherit from the engine', async () => {
    mount(
      true,
      {},
      { dash: { include_video: false, download_concurrency: 2 } },
    );
    expect(
      await screen.findByRole('combobox', { name: 'Record Video' }),
    ).toHaveTextContent('Use engine setting (Off)');
    expect(
      screen.getByRole('combobox', { name: 'Video Quality' }),
    ).toHaveTextContent('Use engine setting (Highest bandwidth)');
    openSection('Segment downloads');
    expect(
      screen.getByRole('combobox', { name: 'Download Concurrency' }),
    ).toHaveTextContent('Use engine setting (2)');
  });

  it('keeps per-setting resets to the default through an unrelated change', async () => {
    const config = {
      dash: {
        audio_language: null,
        include_video: null,
        video_quality: null,
        download_concurrency: null,
      },
    };
    const { saved } = mount(true, config, {
      dash: { audio_language: 'en', include_video: false },
    });
    expect(
      await screen.findByRole('combobox', { name: 'Preferred Audio Language' }),
    ).toHaveTextContent('Default (Any language)');
    expect(
      screen.getByRole('combobox', { name: 'Video Quality' }),
    ).toHaveTextContent('Default (Highest bandwidth)');
    openSection('Segment downloads');
    await choose('Segment Retries', 'Custom');
    fireEvent.change(
      screen.getByRole('spinbutton', { name: 'Segment Retries' }),
      {
        target: { value: '7' },
      },
    );
    expect(saved()).toEqual({
      dash: { ...config.dash, max_segment_retries: 7 },
    });

    await choose('Preferred Audio Language', 'Use engine setting (en)');
    expect(saved().dash).not.toHaveProperty('audio_language');
    await choose('Preferred Audio Language', 'Default (Any language)');
    expect(saved().dash.audio_language).toBeNull();
  });

  it('shows a whole-section reset and keeps it through one change', async () => {
    const { saved } = mount(
      true,
      { fix_hls: false, dash: null },
      { dash: { download_concurrency: 2, max_segment_retries: 9 } },
    );
    expect(
      await screen.findByRole('combobox', { name: 'Video Quality' }),
    ).toHaveTextContent('Default (Highest bandwidth)');
    expect(saved()).toEqual({ fix_hls: false, dash: null });

    openSection('Segment downloads');
    expect(
      screen.getByRole('combobox', { name: 'Download Concurrency' }),
    ).toHaveTextContent('Default (5)');
    await choose('Download Concurrency', 'Custom');
    const dash = saved().dash;
    expect(dash.download_concurrency).toBe(2);
    // Still reset, not inheriting the engine's 9.
    expect(dash.max_segment_retries).toBeNull();
    expect(dash.video_quality).toBeNull();
  });

  it('refuses an override that leaves no track once merged with the engine', async () => {
    const { form } = mount(true, {}, { dash: { include_video: false } });
    await choose('Record Separate Audio', 'Off');
    expect(await screen.findByText(NO_TRACK)).toBeInTheDocument();
    expect(await form.trigger()).toBe(false);

    await choose('Record Video', 'On');
    await waitFor(() => expect(screen.queryByText(NO_TRACK)).toBeNull());
    expect(await form.trigger()).toBe(true);

    // A reset returns video to its default (on), so audio may stay off.
    await choose('Record Video', 'Default (On)');
    expect(await form.trigger()).toBe(true);
  });
});
