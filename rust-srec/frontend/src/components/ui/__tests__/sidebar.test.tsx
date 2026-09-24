import { act, fireEvent, render, screen } from '@testing-library/react';

import { Sidebar, SidebarProvider, SidebarTrigger } from '../sidebar';

function renderMobileSidebar() {
  render(
    <SidebarProvider>
      <Sidebar>
        <a href="/dashboard">Dashboard</a>
        <a href="/streamers">Streamers</a>
      </Sidebar>
      <SidebarTrigger />
    </SidebarProvider>,
  );
  const trigger = screen.getByRole('button', { name: 'Toggle Sidebar' });
  const drawer = () =>
    document.querySelector<HTMLElement>('[data-mobile="true"]');
  const overlay = () =>
    document.querySelector<HTMLElement>('[data-slot="sidebar-overlay"]');
  return { trigger, drawer, overlay };
}

function openDrawer(trigger: HTMLElement) {
  trigger.focus();
  fireEvent.click(trigger);
}

describe('Sidebar on mobile', () => {
  const originalWidth = window.innerWidth;

  beforeEach(() => {
    vi.useFakeTimers();
    Object.defineProperty(window, 'innerWidth', {
      configurable: true,
      value: 390,
    });
  });

  afterEach(() => {
    vi.useRealTimers();
    Object.defineProperty(window, 'innerWidth', {
      configurable: true,
      value: originalWidth,
    });
    document.documentElement.removeAttribute('style');
  });

  it('renders the closed drawer ahead of the first tap', () => {
    const { drawer } = renderMobileSidebar();
    expect(drawer()).toBeNull();

    act(() => {
      vi.runAllTimers();
    });

    expect(drawer()).toHaveAttribute('data-drawer', 'closed');
    expect(drawer()).toHaveAttribute('aria-modal', 'false');
  });

  it('opens with a transition from off-screen when tapped before it rendered', () => {
    const { trigger, drawer } = renderMobileSidebar();

    openDrawer(trigger);

    expect(drawer()).toHaveAttribute('data-drawer', 'open');
    expect(drawer()?.className).toContain('starting:-translate-x-full');
  });

  it('opens without touching inherited styles on the page', () => {
    const { trigger, drawer } = renderMobileSidebar();
    act(() => {
      vi.runAllTimers();
    });

    openDrawer(trigger);

    expect(drawer()).toHaveAttribute('data-drawer', 'open');
    expect(drawer()).toHaveAttribute('role', 'dialog');
    expect(drawer()).toHaveAttribute('aria-modal', 'true');
    expect(drawer()).toHaveFocus();
    expect(document.documentElement.style.overflow).toBe('hidden');
    expect(document.body.getAttribute('style')).toBeNull();
    expect(document.body).not.toHaveAttribute('data-scroll-locked');
    expect(document.querySelector('[inert]')).toBeNull();
  });

  it('closes on Escape and hands focus back to the trigger', () => {
    const { trigger, drawer } = renderMobileSidebar();
    openDrawer(trigger);

    fireEvent.keyDown(document, { key: 'Escape' });

    expect(drawer()).toHaveAttribute('data-drawer', 'closed');
    expect(trigger).toHaveFocus();
    expect(document.documentElement.style.overflow).toBe('');
  });

  it('leaves Escape to a layer that already handled it', () => {
    const { trigger, drawer } = renderMobileSidebar();
    openDrawer(trigger);

    const event = new KeyboardEvent('keydown', {
      key: 'Escape',
      bubbles: true,
      cancelable: true,
    });
    event.preventDefault();
    act(() => {
      document.dispatchEvent(event);
    });

    expect(drawer()).toHaveAttribute('data-drawer', 'open');
  });

  it('closes when the overlay is tapped', () => {
    const { trigger, drawer, overlay } = renderMobileSidebar();
    openDrawer(trigger);

    fireEvent.click(overlay()!);

    expect(drawer()).toHaveAttribute('data-drawer', 'closed');
  });

  it('keeps Tab focus inside the open drawer', () => {
    const { trigger } = renderMobileSidebar();
    openDrawer(trigger);
    const first = screen.getByRole('link', { name: 'Dashboard' });
    const last = screen.getByRole('link', { name: 'Streamers' });

    fireEvent.keyDown(document, { key: 'Tab' });
    expect(first).toHaveFocus();

    last.focus();
    fireEvent.keyDown(document, { key: 'Tab' });
    expect(first).toHaveFocus();

    fireEvent.keyDown(document, { key: 'Tab', shiftKey: true });
    expect(last).toHaveFocus();

    trigger.focus();
    fireEvent.keyDown(document, { key: 'Tab' });
    expect(first).toHaveFocus();
  });
});
