import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { act, fireEvent, render, screen } from '@testing-library/react';
import { useState } from 'react';

import {
  generateBilibiliQr,
  pollBilibiliQr,
} from '@/server/functions/credentials';
import { BilibiliQrLoginDialog } from '../bilibili-qr-login-dialog';

vi.mock('@/server/functions/credentials', () => ({
  generateBilibiliQr: vi.fn(),
  pollBilibiliQr: vi.fn(),
}));

const pending = { success: false, status: 'not_scanned', message: '' };
const expired = { success: false, status: 'expired', message: 'QR expired' };

/** Mirrors the caller, which passes freshly created callbacks on every render. */
function Harness() {
  const [renders, setRenders] = useState(0);
  return (
    <>
      <button type="button" onClick={() => setRenders(renders + 1)}>
        re-render
      </button>
      <BilibiliQrLoginDialog
        open
        scope={{ type: 'platform', id: 'bilibili' }}
        onOpenChange={() => {}}
        onSuccess={() => {}}
      />
    </>
  );
}

async function advance(ms: number) {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.useFakeTimers();
  vi.mocked(generateBilibiliQr).mockResolvedValue({
    url: 'https://example.invalid/qr',
    auth_code: 'code-1',
  } as any);
  vi.mocked(pollBilibiliQr).mockResolvedValue(pending as any);
});

afterEach(() => {
  vi.useRealTimers();
});

async function renderDialog() {
  render(
    <I18nProvider i18n={setupI18n({ locale: 'en', messages: { en: {} } })}>
      <Harness />
    </I18nProvider>,
  );
  // Let the QR code request settle so polling can start.
  await advance(0);
}

describe('BilibiliQrLoginDialog', () => {
  it('polls while the code is still waiting to be scanned', async () => {
    await renderDialog();
    expect(pollBilibiliQr).toHaveBeenCalledTimes(1);

    await advance(4000);

    expect(pollBilibiliQr).toHaveBeenCalledTimes(3);
  });

  it('stops polling once the code has expired', async () => {
    await renderDialog();
    vi.mocked(pollBilibiliQr).mockResolvedValue(expired as any);

    await advance(2000);
    expect(screen.getByText('QR code expired')).toBeInTheDocument();
    const callsWhenExpired = vi.mocked(pollBilibiliQr).mock.calls.length;

    await advance(10000);

    expect(pollBilibiliQr).toHaveBeenCalledTimes(callsWhenExpired);
  });

  it('does not poll again just because the caller re-rendered', async () => {
    await renderDialog();
    expect(pollBilibiliQr).toHaveBeenCalledTimes(1);

    // The open dialog hides the rest of the page from the accessibility tree.
    fireEvent.click(screen.getByText('re-render'));
    await advance(0);

    expect(pollBilibiliQr).toHaveBeenCalledTimes(1);
  });
});
