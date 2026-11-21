import { zodResolver } from '@hookform/resolvers/zod';
import { act, fireEvent, screen, waitFor } from '@testing-library/react';
import { useForm } from 'react-hook-form';
import { Form } from '@/components/ui/form';
import { PlatformConfigFormSchema } from '@/api/schemas';
import { queryKeys } from '@/api/query-keys';
import { PlatformSpecificTab } from '../../platform-specific-tab';
import { newQueryClient, renderWithProviders } from '@/test/render';

const PlatformFormSchema = PlatformConfigFormSchema.partial();

type Batch = boolean | null | undefined;

const platformValues = (batch_detection: Batch) => ({
  platform_specific_config:
    batch_detection === undefined ? {} : { batch_detection },
});

function batchFrom(data: unknown): unknown {
  if (!data || typeof data !== 'object') return undefined;
  const config = (data as Record<string, unknown>).platform_specific_config;
  if (!config || typeof config !== 'object') return undefined;
  return (config as Record<string, unknown>).batch_detection;
}

function renderPlatformFields(
  batch_detection?: Batch,
  capabilities: string[] = ['weibo_batch_detection'],
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
      <Form {...form}>
        <form onSubmit={form.handleSubmit(onSave)}>
          <PlatformSpecificTab form={form} platformName="weibo" />
          <button type="submit">Save</button>
          <button
            type="button"
            onClick={() => form.reset(platformValues(batch_detection))}
          >
            Cancel
          </button>
        </form>
      </Form>
    );
  }

  renderWithProviders(<PlatformHarness />, { client });
  return { onSave, getValues, isDirty, reset };
}

function batchSwitch() {
  return screen.getByRole('switch', { name: /Batch Detection/ });
}

describe('Weibo platform batch detection', () => {
  it('requires the weibo_batch_detection capability rather than another platform capability', () => {
    renderPlatformFields(undefined, ['douyin_batch_detection']);
    expect(batchSwitch()).toBeDisabled();
  });

  it('keeps unset batch detection clean through mount, cancel, and save, and adopts resets', async () => {
    const { onSave, getValues, isDirty, reset } =
      renderPlatformFields(undefined);
    expect(batchSwitch()).not.toBeDisabled();
    expect(batchSwitch()).not.toBeChecked();
    expect(batchFrom(getValues())).toBeUndefined();
    expect(isDirty()).toBe(false);

    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    await waitFor(() => expect(onSave).toHaveBeenCalledTimes(1));
    expect(batchFrom(onSave.mock.calls[0][0])).toBeUndefined();

    fireEvent.click(batchSwitch());
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    await waitFor(() => expect(onSave).toHaveBeenCalledTimes(2));
    expect(batchFrom(onSave.mock.calls[1][0])).toBe(true);

    act(() => reset(false));
    expect(batchSwitch()).not.toBeChecked();
    expect(batchFrom(getValues())).toBe(false);

    act(() => reset(undefined));
    expect(batchSwitch()).not.toBeChecked();
    expect(batchFrom(getValues())).toBeUndefined();
  });

  it('enforces the strict Weibo schema in raw JSON', async () => {
    const { getValues, onSave } = renderPlatformFields(true);
    fireEvent.click(screen.getByRole('button', { name: 'JSON' }));
    const textarea = screen.getByPlaceholderText('{ ... }');

    fireEvent.change(textarea, { target: { value: '{"unknown":true}' } });
    expect(screen.getByText(/Invalid JSON:/)).toBeInTheDocument();

    fireEvent.change(textarea, {
      target: { value: '{"batch_detection":false}' },
    });
    expect(screen.queryByText(/Invalid JSON:/)).not.toBeInTheDocument();
    expect(batchFrom(getValues())).toBe(false);

    fireEvent.change(textarea, { target: { value: '{}' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    await waitFor(() => expect(onSave).toHaveBeenCalledTimes(1));
    expect(batchFrom(onSave.mock.calls[0][0])).toBeUndefined();
  });
});
