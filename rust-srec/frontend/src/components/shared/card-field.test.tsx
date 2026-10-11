import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import { CardField } from './card-field';

describe('CardField', () => {
  it('truncates both lines and exposes plain text as tooltips', () => {
    render(<CardField label="Webhook URL" value="https://example.com/hook" />);

    const label = screen.getByText('Webhook URL');
    const value = screen.getByText('https://example.com/hook');
    expect(label).toHaveClass('truncate');
    expect(label).toHaveAttribute('title', 'Webhook URL');
    expect(value).toHaveClass('truncate');
    expect(value).toHaveAttribute('title', 'https://example.com/hook');
    expect(value.parentElement).toHaveClass('min-w-0');
  });

  it('puts an explicit tooltip on the tile instead of each line', () => {
    render(<CardField title="crf: 23" label="crf" value={23} />);

    expect(screen.getByTitle('crf: 23')).toHaveClass('min-w-0');
    expect(screen.getByText('crf')).not.toHaveAttribute('title');
    expect(screen.getByText('23')).not.toHaveAttribute('title');
  });

  it('leaves element content without a tooltip', () => {
    render(
      <CardField label={<span>Engine</span>} value={<span>default</span>} />,
    );

    expect(screen.getByText('Engine').parentElement).not.toHaveAttribute(
      'title',
    );
    expect(screen.getByText('default').parentElement).not.toHaveAttribute(
      'title',
    );
  });
});
