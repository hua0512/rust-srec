import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { zodResolver } from '@hookform/resolvers/zod';
import { act, fireEvent, render, screen } from '@testing-library/react';
import { useForm, type UseFormReturn } from 'react-hook-form';
import { z } from 'zod';

import {
  DanmuStatisticsObjectSchema,
  DanmuStatisticsOverrideSchema,
} from '@/api/schemas';
import { Form } from '@/components/ui/form';
import { DanmuStatisticsCard } from '../danmu-statistics-card';
import { danmuStatisticsFormValue } from '../danmu-statistics-value';

type DanmuStatistics = z.infer<typeof DanmuStatisticsObjectSchema>;

const LayerSchema = z.object({
  danmu_statistics: DanmuStatisticsOverrideSchema,
});
type Layer = z.infer<typeof LayerSchema>;

function renderCard(stored: DanmuStatistics | null) {
  let form!: UseFormReturn<Layer>;
  function Harness() {
    form = useForm<Layer>({
      resolver: zodResolver(LayerSchema),
      defaultValues: { danmu_statistics: danmuStatisticsFormValue(stored) },
    });
    return (
      <Form {...form}>
        <span data-testid="dirty">{String(form.formState.isDirty)}</span>
        <DanmuStatisticsCard form={form} />
      </Form>
    );
  }
  render(
    <I18nProvider i18n={setupI18n({ locale: 'en', messages: { en: {} } })}>
      <Harness />
    </I18nProvider>,
  );
  const topChatters = screen
    .getByText('Top chatters to keep')
    .closest('[data-slot="form-item"]')!
    .querySelector('input')!;
  return {
    form: () => form,
    topChatters,
    isDirty: () => screen.getByTestId('dirty').textContent === 'true',
  };
}

async function submitted(form: UseFormReturn<Layer>) {
  let data: Layer | undefined;
  await act(() =>
    form.handleSubmit((values) => {
      data = values;
    })(),
  );
  return data;
}

describe('danmu statistics form value', () => {
  // react-hook-form fills fields under a `null` object with `null`, which the schema rejects.
  it('lets a layer without statistics settings be saved, still inheriting', async () => {
    const { form } = renderCard(null);

    const data = await submitted(form());

    expect(data).toBeDefined();
    expect(data!.danmu_statistics).toBeNull();
  });

  it('keeps the settings a layer states', async () => {
    const { form, topChatters } = renderCard(null);

    fireEvent.change(topChatters, { target: { value: '50' } });

    expect((await submitted(form()))!.danmu_statistics).toEqual({
      top_talkers: 50,
    });
  });

  // The inputs register a key per field; one missing from the starting value reads as a change.
  it('reads as unchanged after a value is typed and cleared again', () => {
    const { topChatters, isDirty } = renderCard({ enabled: true });

    fireEvent.change(topChatters, { target: { value: '50' } });
    expect(isDirty()).toBe(true);
    fireEvent.change(topChatters, { target: { value: '' } });

    expect(isDirty()).toBe(false);
  });
});

describe('DanmuStatisticsOverrideSchema', () => {
  it('sends an override with nothing set as inheriting', () => {
    expect(
      DanmuStatisticsOverrideSchema.parse(danmuStatisticsFormValue(null)),
    ).toBeNull();
  });

  it('keeps an override with any setting', () => {
    expect(
      DanmuStatisticsOverrideSchema.parse(
        danmuStatisticsFormValue({ enabled: false }),
      ),
    ).toEqual(expect.objectContaining({ enabled: false }));
  });

  it('leaves an inheriting layer as it is', () => {
    expect(DanmuStatisticsOverrideSchema.parse(null)).toBeNull();
    expect(DanmuStatisticsOverrideSchema.parse(undefined)).toBeUndefined();
  });
});
