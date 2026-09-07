import { render } from '@testing-library/react';

const { animate } = vi.hoisted(() => ({ animate: vi.fn() }));

vi.mock('motion', () => ({ animate }));

import { CountUp } from '../count-up';

type UpdateHandler = (latest: number) => void;

function lastOnUpdate(): UpdateHandler {
  const options = animate.mock.calls.at(-1)?.[2] as {
    onUpdate: UpdateHandler;
  };
  return options.onUpdate;
}

const stop = vi.fn();

beforeEach(() => {
  animate.mockClear();
  stop.mockClear();
  animate.mockReturnValue({ stop });
});

describe('CountUp', () => {
  it('animates once when the value changes', () => {
    const { rerender } = render(<CountUp value={0} />);
    expect(animate).not.toHaveBeenCalled();

    rerender(<CountUp value={100} />);

    expect(animate).toHaveBeenCalledTimes(1);
    expect(animate.mock.calls[0].slice(0, 2)).toEqual([0, 100]);
  });

  // Callers pass inline formatters, so a render of the parent changes the formatter's identity.
  // Restarting the tween for that would make the number jump back mid-count.
  it('keeps the running tween when only the formatter identity changes', () => {
    const { rerender } = render(
      <CountUp value={0} formatter={(value) => `${value}`} />,
    );
    rerender(<CountUp value={100} formatter={(value) => `${value}`} />);
    expect(animate).toHaveBeenCalledTimes(1);

    rerender(<CountUp value={100} formatter={(value) => `${value}!`} />);

    expect(animate).toHaveBeenCalledTimes(1);
    expect(stop).not.toHaveBeenCalled();
  });

  it('formats updates with the latest formatter', () => {
    const { container, rerender } = render(
      <CountUp value={0} formatter={() => 'old'} />,
    );
    rerender(<CountUp value={100} formatter={() => 'old'} />);

    rerender(<CountUp value={100} formatter={() => 'new'} />);
    lastOnUpdate()(42);

    expect(container.querySelector('span')?.textContent).toBe('new');
  });
});
