import type { ReactNode } from 'react';
import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { act, renderHook } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { toast } from 'sonner';

import { useCopyToClipboard } from '../use-copy-to-clipboard';

vi.mock('sonner', () => ({ toast: { success: vi.fn(), error: vi.fn() } }));

const i18n = setupI18n({ locale: 'en', messages: { en: {} } });
const wrapper = ({ children }: { children: ReactNode }) => (
  <I18nProvider i18n={i18n}>{children}</I18nProvider>
);

describe('useCopyToClipboard', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    vi.clearAllMocks();
  });

  // Browsers reject the write when the page lacks focus or permission; the
  // person must not be told it worked.
  it('reports a rejected write as a failure', async () => {
    vi.stubGlobal('navigator', {
      clipboard: { writeText: () => Promise.reject(new Error('denied')) },
    });
    const { result } = renderHook(() => useCopyToClipboard(), { wrapper });

    let copied: boolean | undefined;
    await act(async () => {
      copied = await result.current.copy('text', 'Copied');
    });

    expect(copied).toBe(false);
    expect(result.current.copied).toBe(false);
    expect(toast.success).not.toHaveBeenCalled();
    expect(toast.error).toHaveBeenCalledOnce();
  });
});
