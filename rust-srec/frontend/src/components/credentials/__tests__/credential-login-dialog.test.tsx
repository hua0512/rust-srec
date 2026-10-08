import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { act, render, screen } from '@testing-library/react';
import {
  generateCredentialLogin,
  pollCredentialLogin,
} from '@/server/functions/credential-profiles';
import type { CredentialLoginTarget } from '@/api/schemas/credential-profiles';
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
  });
});
afterEach(() => vi.useRealTimers());
async function advance(ms: number) {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
}
function dialog(
  onSuccess = vi.fn(),
  platformName?: string,
  target: CredentialLoginTarget = {
    type: 'replace',
    profile_id: 'account-a',
    expected_version: 3,
  },
) {
  return (
    <I18nProvider i18n={setupI18n({ locale: 'en', messages: { en: {} } })}>
      <CredentialLoginDialog
        target={target}
        accountLabel="Main account"
        platformName={platformName}
        onClose={() => {}}
        onSuccess={onSuccess}
      />
    </I18nProvider>
  );
}

it('sends only the immutable receipt ID when polling and stops after completion', async () => {
  vi.mocked(pollCredentialLogin).mockResolvedValue({ status: 'completed' });
  const success = vi.fn();
  render(dialog(success));
  await advance(1000);
  expect(pollCredentialLogin).toHaveBeenCalledWith({ data: 'receipt-a' });
  expect(success).toHaveBeenCalledOnce();
  await advance(10000);
  expect(pollCredentialLogin).toHaveBeenCalledOnce();
});

it('does not overlap polls and ignores completion after unmount', async () => {
  let finish!: (value: { status: 'completed' }) => void;
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
  await act(async () => finish({ status: 'completed' }));
  expect(success).not.toHaveBeenCalled();
});

it("names the app of the login's platform", async () => {
  vi.mocked(pollCredentialLogin).mockResolvedValue({ status: 'not_scanned' });
  render(dialog(vi.fn(), 'douyu'));
  await advance(0);
  expect(screen.getByText('Scan with the Douyu app')).toBeTruthy();
});

it('names the account it logs in again or adds', async () => {
  vi.mocked(pollCredentialLogin).mockResolvedValue({ status: 'not_scanned' });
  const view = render(dialog());
  expect(
    screen.getByRole('heading', { name: 'Log in again to “Main account”' }),
  ).toBeInTheDocument();
  view.unmount();
  render(
    dialog(vi.fn(), 'bilibili', {
      type: 'create',
      platform_id: 'platform-a',
      label: 'Main account',
    }),
  );
  expect(
    screen.getByRole('heading', { name: 'Log in to “Main account”' }),
  ).toBeInTheDocument();
});

it('opens without focusing its close button', () => {
  vi.mocked(pollCredentialLogin).mockResolvedValue({ status: 'not_scanned' });
  render(dialog());
  expect(screen.getByRole('dialog')).toHaveFocus();
});
