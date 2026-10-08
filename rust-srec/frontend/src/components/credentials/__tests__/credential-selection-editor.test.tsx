import { fireEvent, screen, within } from '@testing-library/react';
import type { CredentialSelection } from '@/api/schemas/credential-profiles';
import { CredentialSelectionEditor } from '../credential-selection-editor';
import {
  accountDetail,
  profile,
  renderCredentials,
} from './credential-test-utils';

const pool: CredentialSelection = {
  mode: 'pool',
  credential_ids: ['account-a', 'missing'],
  strategy: 'priority',
  failover: true,
  max_attempts: 3,
};

function renderEditor(
  onChange = vi.fn(),
  value: CredentialSelection = pool,
  accounts = [accountDetail()],
) {
  renderCredentials(
    <CredentialSelectionEditor
      value={value}
      onChange={onChange}
      accounts={accounts}
    />,
  );
  return onChange;
}

it('lets maximum attempts be typed freely and clamps it on blur', () => {
  const onChange = renderEditor();
  const input = screen.getByLabelText('Maximum attempts (1–10)');
  // Clearing the field to type a new number neither snaps back nor saves.
  fireEvent.change(input, { target: { value: '' } });
  expect(input).toHaveValue(null);
  expect(onChange).not.toHaveBeenCalled();
  // An in-range value is applied as soon as it is typed.
  fireEvent.change(input, { target: { value: '7' } });
  expect(onChange).toHaveBeenLastCalledWith({ ...pool, max_attempts: 7 });
  // An out-of-range value stays as typed until the field loses focus.
  onChange.mockClear();
  fireEvent.change(input, { target: { value: '25' } });
  expect(input).toHaveValue(25);
  expect(onChange).not.toHaveBeenCalled();
  fireEvent.blur(input);
  expect(input).toHaveValue(10);
  expect(onChange).toHaveBeenLastCalledWith({ ...pool, max_attempts: 10 });
});

it('restores the saved value when maximum attempts is left empty', () => {
  const onChange = renderEditor();
  const input = screen.getByLabelText('Maximum attempts (1–10)');
  fireEvent.change(input, { target: { value: '' } });
  fireEvent.blur(input);
  expect(input).toHaveValue(3);
  expect(onChange).not.toHaveBeenCalled();
});

it('names a pool entry whose profile no longer exists without its raw ID', () => {
  const onChange = renderEditor();
  expect(screen.getByText(/Unknown account/)).toBeInTheDocument();
  expect(screen.queryByText(/missing/)).not.toBeInTheDocument();
  fireEvent.click(screen.getAllByRole('button', { name: 'Remove account' })[1]);
  expect(onChange).toHaveBeenCalledWith({
    ...pool,
    credential_ids: ['account-a'],
  });
});

const health = (validity: 'valid' | 'needs_refresh' | 'invalid') => ({
  validity,
});

it('shows each pool member with its health and marks disabled ones', () => {
  renderEditor(
    vi.fn(),
    { ...pool, credential_ids: ['account-a', 'account-b', 'account-c'] },
    [
      accountDetail({ health: health('valid') }),
      accountDetail({
        profile: { ...profile, id: 'account-b', label: 'Account B' },
        health: health('invalid'),
      }),
      accountDetail({
        profile: {
          ...profile,
          id: 'account-c',
          label: 'A rather long account label that does not fit',
          enabled: false,
        },
      }),
    ],
  );
  const [first, second, third] = screen.getAllByRole('listitem');
  expect(within(first).getByRole('img', { name: 'Valid' })).toBeInTheDocument();
  expect(
    within(second).getByRole('img', { name: 'Login invalid' }),
  ).toBeInTheDocument();
  // No health record yet reads as not checked.
  expect(
    within(third).getByRole('img', { name: 'Not checked' }),
  ).toBeInTheDocument();
  expect(within(third).getByText('Disabled')).toBeInTheDocument();
  // A long label stays on one line and keeps its full text in the tooltip.
  expect(
    within(third).getByTitle('A rather long account label that does not fit'),
  ).toHaveClass('truncate');
  expect(screen.queryByText(/— disabled/)).not.toBeInTheDocument();
});

it('mentions that a recording keeps its account only for fixed and pool selections', () => {
  renderEditor(vi.fn(), { mode: 'fixed', credential_id: profile.id });
  expect(
    screen.getByText('A recording keeps using the account it started with.'),
  ).toBeInTheDocument();
});

it.each([{ mode: 'inherit' as const }, { mode: 'none' as const }])(
  'does not mention recordings keeping an account for $mode',
  (value) => {
    renderEditor(vi.fn(), value);
    expect(
      screen.queryByText(
        'A recording keeps using the account it started with.',
      ),
    ).not.toBeInTheDocument();
  },
);
