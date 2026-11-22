import { zodResolver } from '@hookform/resolvers/zod';
import { act, fireEvent, screen } from '@testing-library/react';
import { useForm, type UseFormReturn } from 'react-hook-form';
import { z } from 'zod';

import {
  DanmuStatisticsObjectSchema,
  DanmuStatisticsOverrideSchema,
} from '@/api/schemas';
import { Form } from '@/components/ui/form';
import { DanmuStatisticsCard } from '../danmu-statistics-card';
import { danmuStatisticsFormValue } from '../danmu-statistics-value';
import { renderWithProviders } from '@/test/render';

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
  renderWithProviders(<Harness />);
  const topChatters = screen
    .getByText('Top chatters to keep')
    .closest('[data-slot="form-item"]')!
    .querySelector('input')!;
  const topWords = screen
    .getByText('Frequent words to keep')
    .closest('[data-slot="form-item"]')!
    .querySelector('input')!;
  return {
    form: () => form,
    topChatters,
    topWords,
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

  it('keeps top_words = 0 instead of collapsing to inherited null', async () => {
    const { form, topWords } = renderCard(null);

    fireEvent.change(topWords, { target: { value: '0' } });

    expect(topWords.value).toBe('0');
    expect((await submitted(form()))!.danmu_statistics).toEqual({
      top_words: 0,
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
