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
import { useForm, type UseFormReturn } from 'react-hook-form';
import { z } from 'zod';
import { Form } from '@/components/ui/form';
import {
  PlatformConfigFormSchema,
  StreamerSpecificConfigFormSchema,
  UpdateTemplateRequestSchema,
} from '@/api/schemas';
import { PlatformOverrideCard } from '@/components/config/templates/tabs/platform-override-card';
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

const schemas = {
  platform: PlatformConfigFormSchema.partial(),
  template: UpdateTemplateRequestSchema,
  streamer: z.object({
    streamer_specific_config: StreamerSpecificConfigFormSchema,
  }),
};
type Scope = keyof typeof schemas;
function values(scope: Scope, quality: number | null | undefined) {
  const options = { quality };
  if (scope === 'platform') return { platform_specific_config: options };
  if (scope === 'template')
    return {
      name: 'Test',
      platform_overrides: { bilibili: { platform_specific_config: options } },
    };
  return { streamer_specific_config: { platform_extras: options } };
}
function qualityFrom(scope: Scope, data: any) {
  if (scope === 'platform') return data.platform_specific_config?.quality;
  if (scope === 'template')
    return data.platform_overrides.bilibili.platform_specific_config?.quality;
  return data.streamer_specific_config.platform_extras?.quality;
}
function renderFields(scope: Scope, quality?: number | null) {
  let form: UseFormReturn<any>;
  const onSave = vi.fn();
  function Harness() {
    form = useForm<any>({
      defaultValues: values(scope, quality),
      resolver: zodResolver(schemas[scope]) as any,
    });
    return (
      <Form {...form}>
        <form onSubmit={form.handleSubmit(onSave)}>
          {scope === 'template' ? (
            <PlatformOverrideCard
              form={form}
              platformName="bilibili"
              onRemove={() => {}}
            />
          ) : scope === 'streamer' ? (
            <StreamerConfiguration form={form} />
          ) : (
            <PlatformSpecificTab form={form} platformName="bilibili" />
          )}
          <button type="submit">Save</button>
          <button
            type="button"
            onClick={() => form.reset(values(scope, quality))}
          >
            Cancel
          </button>
        </form>
      </Form>
    );
  }
  render(
    <I18nProvider i18n={setupI18n({ locale: 'en', messages: { en: {} } })}>
      <Harness />
    </I18nProvider>,
  );
  if (scope === 'template')
    fireEvent.click(screen.getByRole('button', { name: 'Toggle' }));
  return { onSave, form: () => form! };
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
        const { onSave, form } = renderFields(scope, quality);
        const label =
          scope === 'platform' ? 'Default: Dolby Vision (30000)' : 'Inherited';
        expect(
          screen.getByRole('combobox', { name: /Preferred Quality/i }),
        ).toHaveTextContent(label);
        expect(qualityFrom(scope, form().getValues())).toBe(quality);
        expect(form().formState.isDirty).toBe(false);
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
      const { form } = renderFields(scope, 20000);
      const trigger = () =>
        screen.getByRole('combobox', {
          name: /Preferred Quality/i,
        });
      act(() => form().reset(values(scope, 0)));
      expect(trigger()).toHaveTextContent('Lowest (0)');
      act(() => form().reset(values(scope, 127)));
      expect(qualityFrom(scope, form().getValues())).toBe(127);
      expect(trigger()).toHaveTextContent('127');
      act(() => form().reset(values(scope, undefined)));
      expect(qualityFrom(scope, form().getValues())).toBeUndefined();
      expect(trigger()).toHaveTextContent(
        scope === 'platform' ? 'Default:' : 'Inherited',
      );
    });
  },
);
