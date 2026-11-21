import { fireEvent, screen } from '@testing-library/react';
import { useForm, type UseFormReturn } from 'react-hook-form';

import { StreamlinkConfigSchema } from '@/api/schemas/engine';
import { Form } from '@/components/ui/form';
import { StreamlinkForm } from '../streamlink-form';
import { renderWithProviders } from '@/test/render';

type Values = { config?: Record<string, unknown> };

function renderEngine(config: Record<string, unknown>) {
  let form!: UseFormReturn<Values>;
  function Harness() {
    form = useForm<Values>({ defaultValues: { config } });
    return (
      <Form {...form}>
        <StreamlinkForm />
      </Form>
    );
  }
  renderWithProviders(<Harness />);
  return () =>
    JSON.parse(
      JSON.stringify(StreamlinkConfigSchema.parse(form.getValues('config'))),
    );
}

describe('Streamlink FFmpeg path field', () => {
  it('saves an entered path literally and a cleared one as the environment default', () => {
    const saved = renderEngine({ quality: 'best' });
    const input = screen.getByLabelText('FFmpeg Path');
    expect(input).toHaveAttribute(
      'placeholder',
      'Default: FFMPEG_PATH or ffmpeg',
    );
    fireEvent.change(input, {
      target: { value: ' C:\\Media Tools\\ffmpeg.exe ' },
    });
    expect(saved().ffmpeg_path).toBe(' C:\\Media Tools\\ffmpeg.exe ');
    fireEvent.change(input, { target: { value: '' } });
    expect(saved()).not.toHaveProperty('ffmpeg_path');
  });

  it.each([
    ['C:\\Media Tools\\ffmpeg.exe', 'C:\\Media Tools\\ffmpeg.exe'],
    // A null path is unset, which also uses the environment.
    [null, undefined],
    // The backend reads empty text as a path; only the user changes it.
    ['', ''],
    ['   ', '   '],
  ])('keeps a saved %j when other engine settings change', (path, expected) => {
    const saved = renderEngine({ quality: 'best', ffmpeg_path: path });
    fireEvent.change(screen.getByLabelText('Quality'), {
      target: { value: 'worst' },
    });
    expect(saved().quality).toBe('worst');
    expect(saved().ffmpeg_path).toBe(expected);
  });
});
