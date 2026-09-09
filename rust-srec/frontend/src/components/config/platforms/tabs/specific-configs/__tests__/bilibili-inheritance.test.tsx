import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { zodResolver } from '@hookform/resolvers/zod';
import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from '@testing-library/react';
import type { ReactNode } from 'react';
import { useForm } from 'react-hook-form';
import type { FieldValues, UseFormReturn } from 'react-hook-form';
import { Form } from '@/components/ui/form';
import {
  PlatformConfigFormSchema,
  StreamerFormSchema,
  StreamerFormValues,
  UpdateTemplateRequestSchema,
} from '@/api/schemas';
import { PlatformOverrideCard } from '@/components/config/templates/tabs/platform-override-card';
import type { TemplateFormValues } from '@/components/config/templates/template-editor';
import { StreamerConfiguration } from '@/components/streamers/config/streamer-configuration';
import { PlatformSpecificTab } from '../../platform-specific-tab';

vi.mock('@/hooks/use-platform-detection', () => ({
  usePlatformDetection: () => ({ platform: 'Bilibili' }),
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

type Quality = number | null | undefined;
type Scope = 'platform' | 'template' | 'streamer';

/** The same Bilibili quality field, as each scope stores it. */
const platformValues = (quality: Quality) => ({
  platform_specific_config: { quality },
});
const templateValues = (quality: Quality) => ({
  name: 'Test',
  platform_overrides: { bilibili: { platform_specific_config: { quality } } },
});
const streamerValues = (quality: Quality) => ({
  name: 'Test',
  url: 'https://live.bilibili.com/1',
  enabled: true,
  streamer_specific_config: { platform_extras: { quality } },
});

/** Reads a property of a submitted or current form value, which arrives untyped. */
function at(value: unknown, key: string): unknown {
  return value && typeof value === 'object'
    ? (value as Record<string, unknown>)[key]
    : undefined;
}

function qualityFrom(scope: Scope, data: unknown): unknown {
  if (scope === 'platform') {
    return at(at(data, 'platform_specific_config'), 'quality');
  }
  if (scope === 'template') {
    const override = at(at(data, 'platform_overrides'), 'bilibili');
    return at(at(override, 'platform_specific_config'), 'quality');
  }
  return at(
    at(at(data, 'streamer_specific_config'), 'platform_extras'),
    'quality',
  );
}

/** The submit and cancel controls the assertions drive, around the fields under test. */
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

/**
 * Each scope binds its own schema and form type, so the harness is per scope and the assertions
 * share only what they read back: the current values, the dirty flag, and a reset.
 */
function renderFields(scope: Scope, quality?: number | null) {
  const onSave = vi.fn();
  let getValues!: () => unknown;
  let isDirty!: () => boolean;
  let reset!: (next: Quality) => void;

  function PlatformHarness() {
    const form = useForm({
      defaultValues: platformValues(quality),
      resolver: zodResolver(PlatformFormSchema),
    });
    getValues = () => form.getValues();
    isDirty = () => form.formState.isDirty;
    reset = (next) => form.reset(platformValues(next));
    return (
      <Shell
        form={form}
        onSave={onSave}
        onCancel={() => form.reset(platformValues(quality))}
      >
        <PlatformSpecificTab form={form} platformName="bilibili" />
      </Shell>
    );
  }

  function TemplateHarness() {
    const form = useForm<TemplateFormValues>({
      defaultValues: templateValues(quality),
      resolver: zodResolver(UpdateTemplateRequestSchema),
    });
    getValues = () => form.getValues();
    isDirty = () => form.formState.isDirty;
    reset = (next) => form.reset(templateValues(next));
    return (
      <Shell
        form={form}
        onSave={onSave}
        onCancel={() => form.reset(templateValues(quality))}
      >
        <PlatformOverrideCard
          form={form}
          platformName="bilibili"
          onRemove={() => {}}
        />
      </Shell>
    );
  }

  function StreamerHarness() {
    const form = useForm<StreamerFormValues>({
      defaultValues: streamerValues(quality),
      resolver: zodResolver(StreamerFormSchema),
    });
    getValues = () => form.getValues();
    isDirty = () => form.formState.isDirty;
    reset = (next) => form.reset(streamerValues(next));
    return (
      <Shell
        form={form}
        onSave={onSave}
        onCancel={() => form.reset(streamerValues(quality))}
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

  render(
    <I18nProvider i18n={setupI18n({ locale: 'en', messages: { en: {} } })}>
      <Harness />
    </I18nProvider>,
  );
  if (scope === 'template')
    fireEvent.click(screen.getByRole('button', { name: 'Toggle' }));
  return { onSave, getValues, isDirty, reset };
}
async function selectQuality(label: string) {
  fireEvent.keyDown(
    screen.getByRole('combobox', { name: /Preferred Quality/i }),
    { key: 'ArrowDown' },
  );
  fireEvent.click(await screen.findByRole('option', { name: label }));
}

beforeAll(() => {
  HTMLElement.prototype.scrollIntoView = vi.fn();
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;
});

describe.each(['platform', 'template', 'streamer'] as const)(
  'Bilibili %s configuration',
  (scope) => {
    it.each([undefined, null])(
      'keeps %j quality unset through mount, cancel, and validated save',
      async (quality) => {
        const { onSave, getValues, isDirty } = renderFields(scope, quality);
        const label =
          scope === 'platform' ? 'Default: Dolby Vision (30000)' : 'Inherited';
        expect(
          screen.getByRole('combobox', { name: /Preferred Quality/i }),
        ).toHaveTextContent(label);
        expect(qualityFrom(scope, getValues())).toBe(quality);
        expect(isDirty()).toBe(false);
        fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
        fireEvent.click(screen.getByRole('button', { name: 'Save' }));
        await waitFor(() => expect(onSave).toHaveBeenCalled());
        expect(qualityFrom(scope, onSave.mock.calls[0][0])).toBe(quality);
      },
    );

    it('persists explicit selections and clearing restores inheritance', async () => {
      const { onSave } = renderFields(scope);
      await selectQuality('4K (20000)');
      fireEvent.click(screen.getByRole('button', { name: 'Save' }));
      await waitFor(() => expect(onSave).toHaveBeenCalledTimes(1));
      expect(qualityFrom(scope, onSave.mock.calls[0][0])).toBe(20000);
      await selectQuality(
        scope === 'platform' ? 'Default: Dolby Vision (30000)' : 'Inherited',
      );
      fireEvent.click(screen.getByRole('button', { name: 'Save' }));
      await waitFor(() => expect(onSave).toHaveBeenCalledTimes(2));
      expect(qualityFrom(scope, onSave.mock.calls[1][0])).toBeNull();
    });

    it('adopts saved resets and retains zero or older numeric selections', () => {
      const { getValues, reset } = renderFields(scope, 20000);
      const trigger = () =>
        screen.getByRole('combobox', {
          name: /Preferred Quality/i,
        });
      act(() => reset(0));
      expect(trigger()).toHaveTextContent('Lowest (0)');
      act(() => reset(127));
      expect(qualityFrom(scope, getValues())).toBe(127);
      expect(trigger()).toHaveTextContent('127');
      act(() => reset(undefined));
      expect(qualityFrom(scope, getValues())).toBeUndefined();
      expect(trigger()).toHaveTextContent(
        scope === 'platform' ? 'Default:' : 'Inherited',
      );
    });
  },
);
