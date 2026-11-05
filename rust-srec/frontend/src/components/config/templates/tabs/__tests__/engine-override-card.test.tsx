import { fireEvent, screen } from '@testing-library/react';
import { useForm, type UseFormReturn } from 'react-hook-form';

import { Form } from '@/components/ui/form';
import { EngineOverrideCard } from '../engine-override-card';
import { renderWithProviders } from '@/test/render';

type Values = { engines_override?: Record<string, unknown> };

function renderCard(onRemove: () => void) {
  let form!: UseFormReturn<Values>;

  function Harness() {
    form = useForm<Values>({ defaultValues: {} });
    return (
      <Form {...form}>
        <EngineOverrideCard
          engineId="ffmpeg-1"
          engineName="FFmpeg"
          engineType="FFMPEG"
          onRemove={onRemove}
        />
      </Form>
    );
  }

  renderWithProviders(<Harness />);
}

describe('engine override card', () => {
  it('names the icon-only remove control', () => {
    const onRemove = vi.fn();
    renderCard(onRemove);

    fireEvent.click(
      screen.getByRole('button', { name: 'Remove engine override' }),
    );

    expect(onRemove).toHaveBeenCalledTimes(1);
  });
});
