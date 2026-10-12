import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { fireEvent, render, screen, within } from '@testing-library/react';
import { zodResolver } from '@hookform/resolvers/zod';
import { useForm, type UseFormReturn } from 'react-hook-form';
import { z } from 'zod';

import {
  MesioConfigOverrideSchema,
  MesioConfigSchema,
} from '@/api/schemas/engine';
import { Form } from '@/components/ui/form';
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

function mountOverride(
  override: Record<string, unknown>,
  engineConfig: Record<string, unknown> = {},
) {
  let form!: UseFormReturn<Values>;
  // As the engines API returns it, including settings the form does not show.
  const engine = engineConfig;
  function Harness() {
    form = useForm<Values>({
      resolver: zodResolver(
        z.object({
          engines_override: z
            .record(z.string(), MesioConfigOverrideSchema)
            .optional(),
        }),
      ) as never,
      defaultValues: { engines_override: { mesio: override } },
    });
    return (
      <Form {...form}>
        <EngineOverrideCard
          engineId="mesio"
          engineName="Mesio"
          engineType="MESIO"
          engineConfig={engine}
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
  return {
    form,
    saved: () =>
      JSON.parse(
        JSON.stringify(
          MesioConfigOverrideSchema.parse(
            form.getValues('engines_override.mesio'),
          ),
        ),
      ),
  };
}

function mountEngine(config: Record<string, unknown> = {}) {
  let form!: UseFormReturn<Values>;
  function Harness() {
    form = useForm<Values>({
      defaultValues: { config: MesioConfigSchema.parse(config) },
    });
    return (
      <Form {...form}>
        <MesioForm />
      </Form>
    );
  }
  render(
    <I18nProvider i18n={setupI18n({ locale: 'en', messages: { en: {} } })}>
      <Harness />
    </I18nProvider>,
  );
  return {
    saved: () =>
      JSON.parse(
        JSON.stringify(MesioConfigSchema.parse(form.getValues('config'))),
      ),
  };
}

function openTab(name: RegExp) {
  fireEvent.mouseDown(screen.getByRole('tab', { name }), {
    button: 0,
    ctrlKey: false,
  });
}

function combobox(name: string) {
  return screen.getByRole('combobox', { name });
}

async function choose(select: string, option: string) {
  fireEvent.keyDown(combobox(select), { key: 'Enter' });
  fireEvent.click(await screen.findByRole('option', { name: option }));
}

async function options(select: string) {
  fireEvent.keyDown(combobox(select), { key: 'Enter' });
  const listbox = await screen.findByRole('listbox');
  const names = within(listbox)
    .getAllByRole('option')
    .map((option) => option.textContent);
  fireEvent.keyDown(listbox, { key: 'Escape' });
  return names;
}

describe('Mesio template override', () => {
  it('shows what an untouched override inherits and saves nothing', () => {
    const { saved } = mountOverride(
      {},
      { fix_hls: false, hls: { base: { connect_timeout_ms: 1234 } } },
    );
    expect(combobox('Fix HLS Discontinuities')).toHaveTextContent(
      'Use engine setting (Off)',
    );
    expect(combobox('Fix FLV Streams')).toHaveTextContent(
      'Use engine setting (On)',
    );

    // HLS settings apply to downloads whether or not HLS fixing is on.
    expect(screen.getByRole('tab', { name: /HLS Tuning/ })).toBeEnabled();
    openTab(/HLS Tuning/);
    expect(combobox('Connect Timeout (ms)')).toHaveTextContent(
      'Use engine setting (1234)',
    );
    expect(combobox('Read Timeout (ms)')).toHaveTextContent(
      'Use engine setting (30000)',
    );
    expect(saved()).toEqual({});
  });

  it('writes a reset, a custom value and back to inheriting', async () => {
    const { saved } = mountOverride(
      {},
      { hls: { base: { connect_timeout_ms: 1234 } } },
    );
    openTab(/HLS Tuning/);

    await choose('Connect Timeout (ms)', 'Default (30000)');
    expect(saved().hls.base.connect_timeout_ms).toBeNull();

    await choose('Connect Timeout (ms)', 'Custom');
    const input = screen.getByRole('spinbutton', {
      name: 'Connect Timeout (ms)',
    });
    expect(input).toHaveValue(1234);
    fireEvent.change(input, { target: { value: '5000' } });
    expect(saved().hls.base.connect_timeout_ms).toBe(5000);

    await choose('Connect Timeout (ms)', 'Use engine setting (1234)');
    expect(saved().hls.base).not.toHaveProperty('connect_timeout_ms');
  });

  it('gives on/off settings the override states', async () => {
    const { saved } = mountOverride(
      {},
      { hls: { base: { force_ipv4: true } } },
    );
    openTab(/HLS Tuning/);
    expect(combobox('Force IPv4 (Default: Off)')).toHaveTextContent(
      'Use engine setting (On)',
    );
    await choose('Force IPv4 (Default: Off)', 'Default (Off)');
    expect(saved().hls.base.force_ipv4).toBeNull();
    await choose('Force IPv4 (Default: Off)', 'Off');
    expect(saved().hls.base.force_ipv4).toBe(false);
  });

  it('keeps the rest of a reset group reset when one field changes', async () => {
    const { saved } = mountOverride(
      { hls: null },
      { hls: { base: { connect_timeout_ms: 1234, read_timeout_ms: 999 } } },
    );
    openTab(/HLS Tuning/);
    expect(combobox('Connect Timeout (ms)')).toHaveTextContent(
      'Default (30000)',
    );
    expect(saved()).toEqual({ hls: null });

    await choose('Connect Timeout (ms)', 'Custom');
    const hls = saved().hls;
    expect(hls.base.connect_timeout_ms).toBe(1234);
    // Still reset, not inheriting the engine's 999.
    expect(hls.base.read_timeout_ms).toBeNull();
    expect(hls.playlist_config).toBeNull();
    expect(hls.output_config).toBeNull();
  });

  it('also resets the settings the form does not show', async () => {
    const { saved } = mountOverride(
      { hls: null },
      { hls: { fetcher_config: { progress_emit_min_bytes: 1 } } },
    );
    openTab(/HLS Tuning/);
    fireEvent.click(screen.getByRole('button', { name: 'Fetcher' }));
    await choose('Max Segment Retries', 'Custom');
    expect(saved().hls.fetcher_config).toMatchObject({
      max_segment_retries: 3,
      progress_emit_min_bytes: null,
    });
  });

  it('refuses to save a custom value left empty', async () => {
    const { form } = mountOverride({});
    openTab(/HLS Tuning/);
    await choose('Connect Timeout (ms)', 'Custom');
    fireEvent.change(
      screen.getByRole('spinbutton', { name: 'Connect Timeout (ms)' }),
      { target: { value: '' } },
    );
    expect(await form.trigger()).toBe(false);
    expect(
      await screen.findByText('Enter a value, or choose another option'),
    ).toBeInTheDocument();
  });

  it('saves a template whose settings hold resets', async () => {
    const { form } = mountOverride({
      buffer_size: null,
      fix_hls: null,
      fix_flv: null,
      flv_fix: { duplicate_tag_filter_config: null },
      hls: {
        base: null,
        output_config: {
          buffer_limits: null,
          live_max_overall_stall_duration_ms: null,
        },
      },
    });
    expect(await form.trigger()).toBe(true);
    expect(combobox('Fix HLS Discontinuities')).toHaveTextContent(
      'Default (On)',
    );
  });

  it('follows the effective FLV fixing for the FLV tab', async () => {
    const { saved } = mountOverride({}, { fix_flv: false });
    expect(screen.getByRole('tab', { name: /FLV Tuning/ })).toBeDisabled();
    await choose('Fix FLV Streams', 'Default (On)');
    expect(saved().fix_flv).toBeNull();
    expect(screen.getByRole('tab', { name: /FLV Tuning/ })).toBeEnabled();
    await choose('Fix FLV Streams', 'Off');
    expect(saved().fix_flv).toBe(false);
    expect(screen.getByRole('tab', { name: /FLV Tuning/ })).toBeDisabled();
  });

  it('cannot disable a timeout, which only the engine can', async () => {
    const { saved } = mountOverride(
      {},
      { hls: { output_config: { live_max_overall_stall_duration_ms: null } } },
    );
    openTab(/HLS Tuning/);
    fireEvent.click(screen.getByRole('button', { name: 'Output' }));
    expect(await options('Max Stall Duration (ms)')).toEqual([
      'Use engine setting (Disabled)',
      'Default (60000)',
      'Custom',
    ]);
    expect(await options('VOD Segment Timeout (ms)')).toEqual([
      'Use engine setting (Disabled)',
      'Default (Disabled)',
      'Custom',
    ]);
    await choose('Max Stall Duration (ms)', 'Custom');
    expect(saved().hls.output_config.live_max_overall_stall_duration_ms).toBe(
      60000,
    );
  });

  it('offers the override states for policies', async () => {
    const { saved } = mountOverride(
      {},
      {
        hls: {
          output_config: {
            live_gap_strategy: { type: 'skip_after_count', count: 3 },
          },
        },
      },
    );
    openTab(/HLS Tuning/);
    fireEvent.click(screen.getByRole('button', { name: 'Output' }));
    expect(combobox('Live Gap Strategy')).toHaveTextContent(
      'Use engine setting (Skip after count (3))',
    );
    await choose(
      'Live Gap Strategy',
      'Default (Skip after both (10, 5000 ms))',
    );
    expect(saved().hls.output_config.live_gap_strategy).toBeNull();
    await choose('Live Gap Strategy', 'Wait indefinitely');
    expect(saved().hls.output_config.live_gap_strategy).toEqual({
      type: 'wait_indefinitely',
    });
  });
});

describe('Mesio engine settings', () => {
  it('keeps switches and the disabled timeout state', async () => {
    const { saved } = mountEngine({ fix_hls: false });
    expect(
      screen.getByRole('switch', { name: 'Fix HLS Discontinuities' }),
    ).not.toBeChecked();
    expect(screen.getByRole('tab', { name: /HLS Tuning/ })).toBeEnabled();

    openTab(/HLS Tuning/);
    fireEvent.click(screen.getByRole('button', { name: 'Output' }));
    await choose('VOD Segment Timeout (ms)', 'Disabled');
    expect(saved().hls.output_config.vod_segment_timeout_ms).toBeNull();
    await choose('VOD Segment Timeout (ms)', 'Custom');
    expect(saved().hls.output_config.vod_segment_timeout_ms).toBe(30000);
  });

  it('writes plain values with no override states', async () => {
    const { saved } = mountEngine();
    openTab(/HLS Tuning/);
    fireEvent.change(
      screen.getByRole('spinbutton', { name: 'Connect Timeout (ms)' }),
      { target: { value: '7000' } },
    );
    expect(saved().hls.base.connect_timeout_ms).toBe(7000);
    expect(await options('HTTP Version Preference')).toEqual([
      'Auto (Default)',
      'HTTP/2 Only',
      'HTTP/1.1 Only',
    ]);
  });
});
