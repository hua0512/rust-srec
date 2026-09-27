import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { fireEvent, render, screen } from '@testing-library/react';
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

type Values = {
  config?: { enable_lossless_cutting?: boolean };
  engines_override?: Record<string, { enable_lossless_cutting?: boolean }>;
};

beforeAll(() => {
  HTMLElement.prototype.scrollIntoView = vi.fn();
});

describe.each([
  {
    type: 'FFMPEG',
    Component: FfmpegForm,
    schema: FfmpegConfigSchema,
    overrideSchema: FfmpegConfigOverrideSchema,
  },
  {
    type: 'STREAMLINK',
    Component: StreamlinkForm,
    schema: StreamlinkConfigSchema,
    overrideSchema: StreamlinkConfigOverrideSchema,
  },
])(
  '$type lossless cutting configuration',
  ({ type, Component, schema, overrideSchema }) => {
    function mount(value?: boolean, override = false) {
      let form!: UseFormReturn<Values>;
      const config =
        value === undefined ? {} : { enable_lossless_cutting: value };
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
      expect(screen.getByRole('note')).toHaveTextContent(
        'Recording uses temporary chunks',
      );
      expect(screen.getByRole('note')).toHaveTextContent(
        'extra disk space and I/O',
      );
      expect(screen.getByRole('note')).toHaveTextContent(
        'Final files are available only after this step finishes.',
      );
      fireEvent.click(toggle);
      expect(saved().enable_lossless_cutting).toBe(true);
      fireEvent.click(toggle);
      expect(saved().enable_lossless_cutting).toBe(false);
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

    it.each(['false', null])(
      'rejects a malformed opt-in value: %s',
      (value) => {
        expect(
          schema.safeParse({ enable_lossless_cutting: value }).success,
        ).toBe(false);
        expect(
          overrideSchema.safeParse({ enable_lossless_cutting: value }).success,
        ).toBe(false);
      },
    );
  },
);
