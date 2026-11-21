import { cleanup, screen } from '@testing-library/react';
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
import { renderWithProviders } from '@/test/render';

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
    function mount(
      value?: boolean,
      override = false,
      args?: string[],
      engineConfig?: EngineValues,
    ) {
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
                engineConfig={engineConfig}
                onRemove={vi.fn()}
              />
            ) : (
              <Component />
            )}
          </Form>
        );
      }
      renderWithProviders(<Harness />);
      return () =>
        override
          ? overrideSchema.parse(form.getValues('engines_override.engine'))
          : schema.parse(form.getValues('config'));
    }

    it('evaluates an override together with the engine settings it inherits', () => {
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
      cleanup();
      // Both inherited from the engine.
      mount(undefined, true, undefined, {
        enable_lossless_cutting: true,
        [argsField]: ['-flag'],
      });
      expect(details()).toBeInTheDocument();
      expect(screen.getByRole('alert')).toHaveTextContent(warning);
      cleanup();
      // The override clears the engine's arguments.
      mount(undefined, true, [], {
        enable_lossless_cutting: true,
        [argsField]: ['-flag'],
      });
      expect(screen.queryByRole('alert')).toBeNull();
    });
  },
);
