import { useForm, type FieldValues } from 'react-hook-form';

import { Form } from '@/components/ui/form';
import { ExecuteConfigForm } from '../execute-config-form';
import { renderWithProviders } from '@/test/render';

function renderForm() {
  function Harness() {
    const form = useForm<FieldValues>({
      defaultValues: { command: 'ffmpeg -i {input} -c copy {output}' },
    });
    return (
      <Form {...form}>
        <ExecuteConfigForm control={form.control} />
      </Form>
    );
  }
  return renderWithProviders(<Harness />);
}

describe('ExecuteConfigForm', () => {
  it('lists the placeholders', () => {
    const { getByText } = renderForm();

    expect(getByText('{input}')).toBeInTheDocument();
  });

  // A paragraph cannot contain block content: the browser closes it early and
  // the server and client end up with different trees.
  it('keeps the placeholder legend out of the paragraph text', () => {
    const { container } = renderForm();

    expect(container.querySelector('p div')).toBeNull();
  });

  // The command box points at the description slot for its accessible
  // description, so something has to occupy it.
  it('describes the command box', () => {
    const { container } = renderForm();
    const described = container
      .querySelector('textarea')!
      .getAttribute('aria-describedby')!
      .split(' ');

    expect(described.some((id) => container.querySelector(`p#${id}`))).toBe(
      true,
    );
  });
});
