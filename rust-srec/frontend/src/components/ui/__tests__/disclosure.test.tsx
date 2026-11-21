import { fireEvent, render, screen } from '@testing-library/react';
import { useState } from 'react';

import {
  Disclosure,
  DisclosureContent,
  DisclosureTrigger,
} from '../disclosure';

function parts(name: string) {
  const trigger = screen.getByRole('button', { name });
  const content = document.getElementById(
    trigger.getAttribute('aria-controls') ?? '',
  );
  if (!content) throw new Error(`no content for ${name}`);
  const root = trigger.closest<HTMLElement>('[data-slot="disclosure"]');
  return { trigger, content, root };
}

function expectOpen(name: string, open: boolean) {
  const { trigger, content, root } = parts(name);
  expect(trigger).toHaveAttribute('aria-expanded', String(open));
  expect(root).toHaveAttribute('data-state', open ? 'open' : 'closed');
  expect(content).toHaveAttribute('data-state', open ? 'open' : 'closed');
  // jsdom has no `inert` property, so check the reflected attribute.
  expect(content.hasAttribute('inert')).toBe(!open);
}

describe('Disclosure', () => {
  it('toggles when uncontrolled and keeps closed content mounted but inert', () => {
    render(
      <Disclosure>
        <DisclosureTrigger>Details</DisclosureTrigger>
        <DisclosureContent className="p-4">Body text</DisclosureContent>
      </Disclosure>,
    );
    expectOpen('Details', false);
    expect(screen.getByText('Body text')).toHaveClass('p-4');

    fireEvent.click(screen.getByRole('button', { name: 'Details' }));
    expectOpen('Details', true);

    fireEvent.click(screen.getByRole('button', { name: 'Details' }));
    expectOpen('Details', false);
    expect(screen.getByText('Body text')).toBeInTheDocument();
  });

  it('only changes when a controlling parent updates open', () => {
    const onOpenChange = vi.fn();
    const { rerender } = render(
      <Disclosure open={false} onOpenChange={onOpenChange}>
        <DisclosureTrigger>Details</DisclosureTrigger>
        <DisclosureContent>Body text</DisclosureContent>
      </Disclosure>,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Details' }));
    expect(onOpenChange).toHaveBeenCalledWith(true);
    expectOpen('Details', false);

    rerender(
      <Disclosure open onOpenChange={onOpenChange}>
        <DisclosureTrigger>Details</DisclosureTrigger>
        <DisclosureContent>Body text</DisclosureContent>
      </Disclosure>,
    );
    expectOpen('Details', true);

    function Parent() {
      const [open, setOpen] = useState(false);
      return (
        <Disclosure open={open} onOpenChange={setOpen}>
          <DisclosureTrigger>Linked</DisclosureTrigger>
          <DisclosureContent>Linked body</DisclosureContent>
        </Disclosure>
      );
    }
    render(<Parent />);
    fireEvent.click(screen.getByRole('button', { name: 'Linked' }));
    expectOpen('Linked', true);
  });
});
