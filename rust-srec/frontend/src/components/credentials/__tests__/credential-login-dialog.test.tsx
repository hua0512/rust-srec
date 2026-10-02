import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { act, render } from '@testing-library/react';
import {
  generateCredentialLogin,
  pollCredentialLogin,
} from '@/server/functions/credential-profiles';
import { CredentialLoginDialog } from '../credential-login-dialog';

vi.mock('@/server/functions/credential-profiles', () => ({
  generateCredentialLogin: vi.fn(),
  pollCredentialLogin: vi.fn(),
}));

beforeEach(() => {
  vi.useFakeTimers();
  vi.clearAllMocks();
  vi.mocked(generateCredentialLogin).mockResolvedValue({
    login_id: 'receipt-a',
    url: 'https://example.invalid/qr',
    expires_at: Date.now() + 300000,
  });
});
afterEach(() => vi.useRealTimers());
async function advance(ms: number) {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
}
function dialog(onSuccess = vi.fn()) {
  return (
    <I18nProvider i18n={setupI18n({ locale: 'en', messages: { en: {} } })}>
      <CredentialLoginDialog
        target={{
          type: 'replace',
          profile_id: 'account-a',
          expected_version: 3,
        }}
        onClose={() => {}}
        onSuccess={onSuccess}
      />
    </I18nProvider>
  );
}

it('sends only the immutable receipt ID when polling and stops after completion', async () => {
  vi.mocked(pollCredentialLogin).mockResolvedValue({
    status: 'completed',
    profile_id: 'account-a',
    version: 4,
  });
  const success = vi.fn();
  render(dialog(success));
  await advance(1000);
  expect(pollCredentialLogin).toHaveBeenCalledWith({ data: 'receipt-a' });
  expect(success).toHaveBeenCalledOnce();
  await advance(10000);
  expect(pollCredentialLogin).toHaveBeenCalledOnce();
});

it('does not overlap polls and ignores completion after unmount', async () => {
  let finish!: (value: {
    status: 'completed';
    profile_id: string;
    version: number;
  }) => void;
  vi.mocked(pollCredentialLogin).mockReturnValue(
    new Promise((resolve) => {
      finish = resolve;
    }),
  );
  const success = vi.fn();
  const view = render(dialog(success));
  await advance(11000);
  expect(pollCredentialLogin).toHaveBeenCalledOnce();
  view.unmount();
  await act(async () =>
    finish({ status: 'completed', profile_id: 'account-a', version: 4 }),
  );
  expect(success).not.toHaveBeenCalled();
});
