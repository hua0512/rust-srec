import { zodResolver } from '@hookform/resolvers/zod';
import {
  act,
  fireEvent,
  screen,
  waitFor,
  within,
} from '@testing-library/react';
import type { ReactNode } from 'react';
import { useForm } from 'react-hook-form';
import type { FieldValues, UseFormReturn } from 'react-hook-form';
import { Form } from '@/components/ui/form';
import {
  PlatformConfigFormSchema,
  StreamerFormSchema,
  type StreamerFormValues,
  UpdateTemplateRequestSchema,
} from '@/api/schemas';
import { queryKeys } from '@/api/query-keys';
import { PlatformOverrideCard } from '@/components/config/templates/tabs/platform-override-card';
import type { TemplateFormValues } from '@/components/config/templates/template-editor';
import { StreamerConfiguration } from '@/components/streamers/config/streamer-configuration';
import { PlatformSpecificTab } from '../../platform-specific-tab';
import { newQueryClient, renderWithProviders } from '@/test/render';

vi.mock('@/hooks/use-platform-detection', () => ({
  usePlatformDetection: () => ({ platform: 'Douyin' }),
}));
vi.mock('@/components/config/shared-config-editor', () => ({
  SharedConfigEditor: ({
    extraTabs,
  }: {
    extraTabs: { value: string; content: React.ReactNode }[];
  }) =>
    extraTabs.find((tab) => ['specific', 'platform'].includes(tab.value))
      ?.content,
}));

const PlatformFormSchema = PlatformConfigFormSchema.partial();

type Batch = boolean | null | undefined;
type Scope = 'platform' | 'template' | 'streamer';

const platformValues = (batch_detection: Batch) => ({
  platform_specific_config:
    batch_detection === undefined ? {} : { batch_detection },
});
const templateValues = (batch_detection: Batch) => ({
  name: 'Test',
  platform_overrides: {
    douyin: {
      platform_specific_config:
        batch_detection === undefined ? {} : { batch_detection },
    },
  },
});
const streamerValues = (batch_detection: Batch) => ({
  name: 'Test',
  url: 'https://live.douyin.com/123456',
  enabled: true,
  streamer_specific_config: {
    platform_extras: batch_detection === undefined ? {} : { batch_detection },
  },
});

function at(value: unknown, key: string): unknown {
  return value && typeof value === 'object'
    ? (value as Record<string, unknown>)[key]
    : undefined;
}

function batchFrom(scope: Scope, data: unknown): unknown {
  if (scope === 'platform') {
    return at(at(data, 'platform_specific_config'), 'batch_detection');
  }
  if (scope === 'template') {
    const override = at(at(data, 'platform_overrides'), 'douyin');
    return at(at(override, 'platform_specific_config'), 'batch_detection');
  }
  return at(
    at(at(data, 'streamer_specific_config'), 'platform_extras'),
    'batch_detection',
  );
}

function Shell<TFieldValues extends FieldValues>({
  form,
  onSave,
  onCancel,
  children,
}: {
  form: UseFormReturn<TFieldValues>;
  onSave: (data: TFieldValues) => void;
  onCancel: () => void;
  children: ReactNode;
}) {
  return (
    <Form {...form}>
      <form onSubmit={form.handleSubmit(onSave)}>
        {children}
        <button type="submit">Save</button>
        <button type="button" onClick={onCancel}>
          Cancel
        </button>
      </form>
    </Form>
  );
}

function renderFields(
  scope: Scope,
  batch_detection?: Batch,
  capabilities: string[] = ['douyin_batch_detection'],
) {
  const onSave = vi.fn();
  let getValues!: () => unknown;
  let isDirty!: () => boolean;
  let reset!: (next: Batch) => void;

  const client = newQueryClient();
  client.setQueryData(queryKeys.health, {
    status: 'healthy',
    version: '0.8.0',
    uptime_secs: 1,
    cpu_usage: 0,
    memory_usage: 0,
    components: [],
    capabilities,
  });

  function PlatformHarness() {
    const form = useForm({
      defaultValues: platformValues(batch_detection),
      resolver: zodResolver(PlatformFormSchema),
    });
    getValues = () => form.getValues();
    isDirty = () => form.formState.isDirty;
    reset = (next) => form.reset(platformValues(next));
    return (
      <Shell
        form={form}
        onSave={onSave}
        onCancel={() => form.reset(platformValues(batch_detection))}
      >
        <PlatformSpecificTab form={form} platformName="douyin" />
      </Shell>
    );
  }

  function TemplateHarness() {
    const form = useForm<TemplateFormValues>({
      defaultValues: templateValues(batch_detection),
      resolver: zodResolver(UpdateTemplateRequestSchema),
    });
    getValues = () => form.getValues();
    isDirty = () => form.formState.isDirty;
    reset = (next) => form.reset(templateValues(next));
    return (
      <Shell
        form={form}
        onSave={onSave}
        onCancel={() => form.reset(templateValues(batch_detection))}
      >
        <PlatformOverrideCard
          form={form}
          platformName="douyin"
          onRemove={() => {}}
        />
      </Shell>
    );
  }

  function StreamerHarness() {
    const form = useForm<StreamerFormValues>({
      defaultValues: streamerValues(batch_detection),
      resolver: zodResolver(StreamerFormSchema),
    });
    getValues = () => form.getValues();
    isDirty = () => form.formState.isDirty;
    reset = (next) => form.reset(streamerValues(next));
    return (
      <Shell
        form={form}
        onSave={onSave}
        onCancel={() => form.reset(streamerValues(batch_detection))}
      >
        <StreamerConfiguration form={form} />
      </Shell>
    );
  }

  const Harness =
    scope === 'platform'
      ? PlatformHarness
      : scope === 'template'
        ? TemplateHarness
        : StreamerHarness;

  renderWithProviders(<Harness />, { client });
  if (scope === 'template')
    fireEvent.click(
      screen.getByRole('button', { name: /Platform Override Settings/ }),
    );
  return { onSave, getValues, isDirty, reset };
}

function batchControl(scope: Scope, value: boolean | null) {
  if (scope === 'platform') {
    return screen.getByRole('switch', { name: /Batch Detection/ });
  }
  return within(
    screen.getByRole('radiogroup', { name: /Batch Detection/ }),
  ).getByRole('radio', {
    name: value === null ? 'Inherit' : value ? 'On' : 'Off',
  });
}

describe.each(['platform', 'template', 'streamer'] as const)(
  'Douyin %s batch detection',
  (scope) => {
    it('disables the control when the backend does not report the capability', () => {
      renderFields(scope, undefined, []);
      if (scope === 'platform') {
        expect(batchControl(scope, null)).toBeDisabled();
      } else {
        for (const choice of [null, true, false] as const) {
          expect(batchControl(scope, choice)).toBeDisabled();
        }
      }
    });

    it.each([undefined, null])(
      'keeps %j inherited through mount, cancel, and save',
      async (value) => {
        const { onSave, getValues, isDirty } = renderFields(scope, value);
        if (scope === 'platform') {
          expect(batchControl(scope, null)).not.toBeChecked();
        } else {
          expect(batchControl(scope, null)).toBeChecked();
        }
        expect(batchFrom(scope, getValues())).toBe(value);
        expect(isDirty()).toBe(false);
        fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
        fireEvent.click(screen.getByRole('button', { name: 'Save' }));
        await waitFor(() => expect(onSave).toHaveBeenCalled());
        expect(batchFrom(scope, onSave.mock.calls[0][0])).toBe(value);
      },
    );

    it('saves explicit enabling and disabling without losing the override', async () => {
      const { onSave } = renderFields(scope);
      fireEvent.click(batchControl(scope, true));
      fireEvent.click(screen.getByRole('button', { name: 'Save' }));
      await waitFor(() => expect(onSave).toHaveBeenCalledTimes(1));
      expect(batchFrom(scope, onSave.mock.calls[0][0])).toBe(true);
      fireEvent.click(batchControl(scope, false));
      fireEvent.click(screen.getByRole('button', { name: 'Save' }));
      await waitFor(() => expect(onSave).toHaveBeenCalledTimes(2));
      expect(batchFrom(scope, onSave.mock.calls[1][0])).toBe(false);
      if (scope !== 'platform') {
        fireEvent.click(batchControl(scope, null));
        fireEvent.click(screen.getByRole('button', { name: 'Save' }));
        await waitFor(() => expect(onSave).toHaveBeenCalledTimes(3));
        expect(batchFrom(scope, onSave.mock.calls[2][0])).toBeNull();
      }
    });

    it('adopts resets without creating an override', () => {
      const { getValues, reset } = renderFields(scope, true);
      expect(batchControl(scope, true)).toBeChecked();
      act(() => reset(false));
      expect(batchControl(scope, true)).not.toBeChecked();
      expect(batchFrom(scope, getValues())).toBe(false);
      act(() => reset(undefined));
      if (scope === 'platform') {
        expect(batchControl(scope, null)).not.toBeChecked();
      } else {
        expect(batchControl(scope, null)).toBeChecked();
      }
      expect(batchFrom(scope, getValues())).toBeUndefined();
    });
  },
);

it('allows batch detection in raw JSON and removing it restores inheritance', async () => {
  const { getValues, onSave } = renderFields('platform', true);
  fireEvent.click(screen.getByRole('button', { name: 'JSON' }));
  const textarea = screen.getByPlaceholderText('{ ... }');
  fireEvent.change(textarea, {
    target: { value: '{"batch_detection":false}' },
  });
  expect(batchFrom('platform', getValues())).toBe(false);
  fireEvent.change(textarea, { target: { value: '{}' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save' }));
  await waitFor(() => expect(onSave).toHaveBeenCalled());
  expect(batchFrom('platform', onSave.mock.calls[0][0])).toBeUndefined();
});
