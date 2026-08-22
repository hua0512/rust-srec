import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import {
  createFrontendFailure,
  FatalScreen,
  parseBootFailure,
} from './fatal-screen';

describe('parseBootFailure', () => {
  it('reads a structured native boot failure', () => {
    const failure = parseBootFailure({
      stage: 'database',
      kind: 'database_busy',
      title: 'Database unavailable',
      message: 'Close the other process.',
      details: '(code: 5) database is locked',
    });

    expect(failure).toMatchObject({
      origin: 'native',
      stage: 'database',
      kind: 'database_busy',
      title: 'Database unavailable',
    });
  });

  it('keeps legacy string errors recoverable', () => {
    const failure = parseBootFailure('legacy startup error');

    expect(failure).toMatchObject({
      origin: 'native',
      stage: 'backend',
      kind: 'unknown',
      details: 'legacy startup error',
    });
  });

  it('ignores missing and malformed payloads', () => {
    expect(parseBootFailure(null)).toBeNull();
    expect(parseBootFailure({ title: 'Incomplete' })).toBeNull();
  });
});

describe('createFrontendFailure', () => {
  it('constructs a structured frontend error with stack trace', () => {
    const error = new Error('Webview crashed');
    const failure = createFrontendFailure(error);

    expect(failure).toMatchObject({
      origin: 'frontend',
      stage: 'window',
      kind: 'window_failed',
      title: 'The desktop interface could not load',
    });
    expect(failure.details).toContain('Webview crashed');
  });
});

describe('FatalScreen', () => {
  it('offers database recovery actions without a reset action', () => {
    const failure = parseBootFailure({
      stage: 'migrations',
      kind: 'database_corrupt',
      title: 'The database may be damaged',
      message: 'Back up the database before recovery.',
      details: 'database disk image is malformed',
    });

    expect(failure).not.toBeNull();
    render(<FatalScreen failure={failure!} />);

    expect(
      screen.getByRole('button', { name: 'Restart application' }),
    ).toBeVisible();
    expect(
      screen.getByRole('button', { name: 'Open data folder' }),
    ).toBeVisible();
    expect(screen.getByRole('button', { name: 'Open logs' })).toBeVisible();
    expect(screen.getByRole('button', { name: 'Quit' })).toBeVisible();
    expect(screen.queryByRole('button', { name: /reset/i })).toBeNull();
  });

  it('renders contextual troubleshooting guidance for database lock errors', () => {
    const failure = parseBootFailure({
      stage: 'database',
      kind: 'database_busy',
      title: 'Database locked',
      message: 'Another process is accessing the database.',
      details: 'database is locked',
    });

    expect(failure).not.toBeNull();
    render(<FatalScreen failure={failure!} />);

    expect(
      screen.getByText(
        /Another instance of Rust-Srec or an external SQLite browser/i,
      ),
    ).toBeVisible();
  });

  it('uses interface reload for frontend failures', () => {
    render(<FatalScreen failure={createFrontendFailure(new Error('boom'))} />);

    expect(
      screen.getByRole('button', { name: 'Reload interface' }),
    ).toBeVisible();
    expect(
      screen.queryByRole('button', { name: 'Restart application' }),
    ).toBeNull();
  });

  it('copies diagnostic details to clipboard when copy button is clicked', async () => {
    const writeTextMock = vi.fn().mockResolvedValue(undefined);
    Object.assign(navigator, {
      clipboard: {
        writeText: writeTextMock,
      },
    });

    const failure = parseBootFailure({
      stage: 'backend',
      kind: 'unknown',
      title: 'Failed to start engine',
      message: 'Review logs.',
      details: 'Panic in thread main at backend.rs:42',
    });

    render(<FatalScreen failure={failure!} />);

    const copyBtn = screen.getByRole('button', { name: 'Copy details' });
    fireEvent.click(copyBtn);

    await waitFor(() => {
      expect(writeTextMock).toHaveBeenCalledWith(
        expect.stringContaining('Panic in thread main at backend.rs:42'),
      );
    });

    expect(
      screen.getByRole('button', { name: 'Details copied' }),
    ).toBeVisible();
  });

  it('renders technical diagnostics console and details', () => {
    const failure = parseBootFailure({
      stage: 'logging',
      kind: 'logging_failed',
      title: 'Logging directory read-only',
      message: 'Check permissions.',
      details: 'os error 13: Permission denied',
    });

    render(<FatalScreen failure={failure!} />);

    expect(screen.getByText('Technical details')).toBeVisible();
    expect(screen.getByText('startup_diagnostics.log')).toBeInTheDocument();
    expect(
      screen.getByText(/os error 13: Permission denied/),
    ).toBeInTheDocument();
  });
});
