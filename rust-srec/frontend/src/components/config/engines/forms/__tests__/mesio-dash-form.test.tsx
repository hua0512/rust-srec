import { fireEvent, screen, waitFor } from '@testing-library/react';
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
import { renderWithProviders } from '@/test/render';

type Values = {
  config?: Record<string, unknown>;
  engines_override?: Record<string, Record<string, unknown>>;
};

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
  renderWithProviders(<Harness />);
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

async function choose(select: string, option: string) {
  fireEvent.keyDown(screen.getByRole('combobox', { name: select }), {
    key: 'Enter',
  });
  fireEvent.click(await screen.findByRole('option', { name: option }));
}

describe('Mesio DASH settings', () => {
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
});

describe('Mesio DASH template overrides', () => {
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
