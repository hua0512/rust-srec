import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { useForm, type UseFormReturn } from 'react-hook-form';

import {
  FfmpegConfigSchema,
  FfmpegConfigOverrideSchema,
  StreamlinkConfigSchema,
  StreamlinkConfigOverrideSchema,
} from '@/api/schemas/engine';
import { Form } from '@/components/ui/form';
import { EngineOverrideCard } from '../../../templates/tabs/engine-override-card';
import { FfmpegForm } from '../ffmpeg-form';
import { StreamlinkForm } from '../streamlink-form';

type EngineValues = {
  enable_lossless_cutting?: boolean;
  output_args?: string[];
  extra_args?: string[];
};
type Values = {
  config?: EngineValues;
  engines_override?: Record<string, EngineValues>;
};

const details = () =>
  screen.queryByRole('button', { name: 'Recording uses temporary chunks' });

beforeAll(() => {
  HTMLElement.prototype.scrollIntoView = vi.fn();
});

describe.each([
  {
    type: 'FFMPEG',
    argsField: 'output_args',
    warning: 'Custom FFmpeg output arguments are set.',
    Component: FfmpegForm,
    schema: FfmpegConfigSchema,
    overrideSchema: FfmpegConfigOverrideSchema,
  },
  {
    type: 'STREAMLINK',
    argsField: 'extra_args',
    warning: 'Extra Streamlink arguments are set.',
    Component: StreamlinkForm,
    schema: StreamlinkConfigSchema,
    overrideSchema: StreamlinkConfigOverrideSchema,
  },
])(
  '$type lossless cutting configuration',
  ({ type, argsField, warning, Component, schema, overrideSchema }) => {
    function mount(value?: boolean, override = false, args?: string[]) {
      let form!: UseFormReturn<Values>;
      const config: EngineValues = {
        ...(value === undefined ? {} : { enable_lossless_cutting: value }),
        ...(args === undefined ? {} : { [argsField]: args }),
      };
      function Harness() {
        form = useForm<Values>({
          defaultValues: override
            ? { engines_override: { engine: config } }
            : { config: schema.parse(config) },
        });
        return (
          <Form {...form}>
            {override ? (
              <EngineOverrideCard
                engineId="engine"
                engineName={type}
                engineType={type}
                onRemove={vi.fn()}
              />
            ) : (
              <Component />
            )}
          </Form>
        );
      }
      render(
        <I18nProvider i18n={setupI18n({ locale: 'en', messages: { en: {} } })}>
          <Harness />
        </I18nProvider>,
      );
      return () =>
        override
          ? overrideSchema.parse(form.getValues('engines_override.engine'))
          : schema.parse(form.getValues('config'));
    }

    it('defaults old configurations to normal recording and explains the opt-in', () => {
      const saved = mount();
      const toggle = screen.getByRole('switch', {
        name: 'Enable lossless cutting',
      });
      expect(toggle).not.toBeChecked();
      expect(saved().enable_lossless_cutting).toBe(false);
      expect(screen.getByText('Experimental')).toBeInTheDocument();
      expect(
        screen.getByText(/Split recordings without reconnecting/),
      ).toBeInTheDocument();
      expect(details()).toBeNull();
      fireEvent.click(toggle);
      const trigger = details()!;
      const panel = document.getElementById(
        trigger.getAttribute('aria-controls')!,
      )!;
      expect(trigger).toHaveAttribute('aria-expanded', 'false');
      expect(panel).toHaveAttribute('inert');
      fireEvent.click(trigger);
      expect(trigger).toHaveAttribute('aria-expanded', 'true');
      expect(panel).not.toHaveAttribute('inert');
      expect(panel).toHaveTextContent('extra disk space and I/O');
      expect(panel).toHaveTextContent(
        'Final files are available only after this step finishes.',
      );
      expect(panel).toHaveTextContent(
        'Temporary chunks are kept if file finalization fails.',
      );
      fireEvent.click(trigger);
      expect(trigger).toHaveAttribute('aria-expanded', 'false');
      expect(panel).toHaveAttribute('inert');
      expect(saved().enable_lossless_cutting).toBe(true);
      fireEvent.click(toggle);
      expect(saved().enable_lossless_cutting).toBe(false);
      expect(details()).toBeNull();
    });

    it('warns when custom arguments keep lossless cutting from applying', () => {
      mount(false, false, ['-flag']);
      expect(screen.queryByRole('alert')).toBeNull();
      fireEvent.click(
        screen.getByRole('switch', { name: 'Enable lossless cutting' }),
      );
      expect(screen.getByRole('alert')).toHaveTextContent(warning);
      cleanup();
      mount(true, false, []);
      expect(screen.queryByRole('alert')).toBeNull();
    });

    it('evaluates only settings present in an override', () => {
      mount(true, true);
      expect(details()).toBeInTheDocument();
      expect(screen.queryByRole('alert')).toBeNull();
      cleanup();
      mount(true, true, ['-flag']);
      expect(screen.getByRole('alert')).toHaveTextContent(warning);
      cleanup();
      mount(undefined, true, ['-flag']);
      expect(details()).toBeNull();
      expect(screen.queryByRole('alert')).toBeNull();
    });

    it('loads an enabled engine without resetting it', () => {
      const saved = mount(true);
      expect(
        screen.getByRole('switch', { name: 'Enable lossless cutting' }),
      ).toBeChecked();
      expect(saved().enable_lossless_cutting).toBe(true);
    });

    it('keeps inherited, enabled, and explicitly disabled overrides distinct', async () => {
      const saved = mount(undefined, true);
      const choice = screen.getByRole('combobox', {
        name: 'Enable lossless cutting',
      });
      expect(choice).toHaveTextContent('Use engine setting');
      expect(JSON.parse(JSON.stringify(saved()))).not.toHaveProperty(
        'enable_lossless_cutting',
      );
      for (const [label, expected] of [
        ['Enabled', true],
        ['Disabled', false],
        ['Use engine setting', undefined],
      ] as const) {
        fireEvent.keyDown(choice, { key: 'Enter' });
        fireEvent.click(await screen.findByRole('option', { name: label }));
        expect(saved().enable_lossless_cutting).toBe(expected);
      }
      expect(JSON.parse(JSON.stringify(saved()))).not.toHaveProperty(
        'enable_lossless_cutting',
      );
    });
  },
);
