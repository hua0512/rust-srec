import { describe, expect, it, vi } from 'vitest';

import {
  createDesktopErrorController,
  extractErrorMessage,
  isIgnorableRuntimeError,
} from '../runtime-error-handler';

describe('createDesktopErrorController', () => {
  it('invokes onBootFatal at most once for cascading boot-phase errors', () => {
    const onBootFatal = vi.fn();
    const onRuntimeError = vi.fn();
    const controller = createDesktopErrorController({
      onBootFatal,
      onRuntimeError,
    });

    const first = new Error('initial boot failure');
    const second = new Error('cascading failure');

    controller.handleError(first);
    controller.handleError(second);

    expect(onBootFatal).toHaveBeenCalledTimes(1);
    expect(onBootFatal).toHaveBeenCalledWith(first);
    expect(onRuntimeError).not.toHaveBeenCalled();
  });

  it('routes post-mount errors to onRuntimeError instead of onBootFatal', () => {
    const onBootFatal = vi.fn();
    const onRuntimeError = vi.fn();
    const controller = createDesktopErrorController({
      onBootFatal,
      onRuntimeError,
    });

    controller.markMounted();

    const runtimeError = new Error('WebSocket disconnected\nat connect.ts:42');
    controller.handleError(runtimeError);

    expect(onBootFatal).not.toHaveBeenCalled();
    expect(onRuntimeError).toHaveBeenCalledTimes(1);
    expect(onRuntimeError).toHaveBeenCalledWith(
      'WebSocket disconnected',
      runtimeError,
    );
  });

  it('ignores benign ResizeObserver errors both before and after mount', () => {
    const onBootFatal = vi.fn();
    const onRuntimeError = vi.fn();
    const controller = createDesktopErrorController({
      onBootFatal,
      onRuntimeError,
    });

    const chromeNotice = new ErrorEvent('error', {
      message: 'ResizeObserver loop completed with undelivered notifications.',
    });
    const safariNotice = 'ResizeObserver loop limit exceeded';

    expect(isIgnorableRuntimeError(chromeNotice)).toBe(true);
    expect(isIgnorableRuntimeError(safariNotice)).toBe(true);

    controller.handleError(chromeNotice);
    controller.handleError(safariNotice);

    controller.markMounted();

    controller.handleError(new Error(safariNotice));
    controller.handleError(chromeNotice);

    expect(onBootFatal).not.toHaveBeenCalled();
    expect(onRuntimeError).not.toHaveBeenCalled();
  });

  it('deduplicates identical runtime messages within dedupeWindowMs and allows them again after the window elapses', () => {
    let currentTime = 1_000;
    const onBootFatal = vi.fn();
    const onRuntimeError = vi.fn();
    const controller = createDesktopErrorController({
      onBootFatal,
      onRuntimeError,
      now: () => currentTime,
      dedupeWindowMs: 5_000,
    });

    controller.markMounted();

    const first = new Error('Request failed');
    const duplicate = new Error('Request failed');
    const distinct = new Error('Storage full');
    const afterWindow = new Error('Request failed');

    controller.handleError(first);

    currentTime = 3_500;
    controller.handleError(duplicate);
    controller.handleError(distinct);

    currentTime = 5_999;
    controller.handleError(duplicate);

    expect(onRuntimeError).toHaveBeenCalledTimes(2);
    expect(onRuntimeError).toHaveBeenNthCalledWith(1, 'Request failed', first);
    expect(onRuntimeError).toHaveBeenNthCalledWith(2, 'Storage full', distinct);

    currentTime = 6_000;
    controller.handleError(afterWindow);

    expect(onRuntimeError).toHaveBeenCalledTimes(3);
    expect(onRuntimeError).toHaveBeenNthCalledWith(
      3,
      'Request failed',
      afterWindow,
    );
  });
});

describe('extractErrorMessage', () => {
  it('prefers the wrapped error of an ErrorEvent over its prefixed message', () => {
    expect(
      extractErrorMessage(
        new ErrorEvent('error', {
          error: new Error('Inner error'),
          message: 'Uncaught Error: Inner error',
        }),
      ),
    ).toBe('Inner error');
    expect(
      extractErrorMessage(
        new ErrorEvent('error', { message: 'Script error.' }),
      ),
    ).toBe('Script error.');
  });
});
