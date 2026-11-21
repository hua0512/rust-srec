import { useForm } from 'react-hook-form';
import { act, cleanup, screen, waitFor } from '@testing-library/react';
import {
  CredentialSettings,
  CredentialSettingsField,
} from '../credential-settings-field';
import type {
  CredentialOwner,
  CredentialPlatform,
} from '@/api/schemas/credential-profiles';
import {
  mockCredentialApi,
  mockPlatformCapabilities,
  platformOwner,
  profile,
  renderCredentials,
  templateOwner,
} from './credential-test-utils';

vi.mock('@tanstack/react-router', async () => ({
  Link: (await import('@/test/router')).MockLink,
}));

vi.mock('@/server/functions/credential-profiles');

beforeEach(mockCredentialApi);

const rowName = 'Details for Account A';

function settings(scope: CredentialOwner, platformName = 'Bilibili') {
  mockPlatformCapabilities(platformName);
  return (
    <CredentialSettings
      scope={scope}
      platform={{ id: 'platform-a', name: platformName }}
      selection={{ mode: 'fixed', credential_id: profile.id }}
      onSelectionChange={vi.fn()}
    />
  );
}

it('Streamlink accounts are chosen by site or per streamer, not on the platform or a template', async () => {
  for (const [owner, text] of [
    [platformOwner, /each account names the sites it is for/],
    [templateOwner, /chosen by site or on each streamer, not in a template/],
  ] as const) {
    renderCredentials(settings(owner, 'streamlink'));
    expect(
      await screen.findByRole('button', { name: rowName }),
    ).toBeInTheDocument();
    expect(screen.queryByText('Account selection')).not.toBeInTheDocument();
    expect(screen.getByText(text)).toBeInTheDocument();
    cleanup();
  }
  // The platform page still manages Streamlink accounts.
  renderCredentials(settings(platformOwner, 'streamlink'));
  expect(
    await screen.findByRole('button', { name: 'Add account' }),
  ).toBeInTheDocument();
});

type FieldForm = ReturnType<typeof useForm<{ credential_selection?: unknown }>>;

function Field({
  scope,
  platform,
  defaultSelection,
  onForm,
}: {
  scope?: CredentialOwner;
  platform?: CredentialPlatform;
  defaultSelection?: unknown;
  onForm?: (form: FieldForm) => void;
}) {
  const form = useForm<{ credential_selection?: unknown }>({
    defaultValues: { credential_selection: defaultSelection },
  });
  onForm?.(form);
  return (
    <CredentialSettingsField
      form={form}
      name="credential_selection"
      scope={scope}
      platform={platform}
    />
  );
}

it('marks the selection unsaved until the form is saved with it', async () => {
  mockPlatformCapabilities('Bilibili');
  let form: FieldForm | undefined;
  // The saved pool lists its keys in another order than the form builds them.
  const saved = {
    max_attempts: 3,
    failover: true,
    strategy: 'priority',
    credential_ids: [profile.id],
    mode: 'pool',
  };
  renderCredentials(
    <Field
      scope={{ type: 'streamer', streamer_id: 'streamer-a' }}
      platform={{ id: 'platform-a', name: 'Bilibili' }}
      defaultSelection={saved}
      onForm={(value) => {
        form = value;
      }}
    />,
  );
  await screen.findByRole('button', { name: 'Change' });
  expect(screen.queryByText('Unsaved')).not.toBeInTheDocument();
  act(() =>
    form?.setValue(
      'credential_selection',
      { mode: 'none' },
      {
        shouldDirty: true,
      },
    ),
  );
  expect(await screen.findByText('Unsaved')).toBeInTheDocument();
  act(() => form?.reset({ credential_selection: { mode: 'none' } }));
  await waitFor(() =>
    expect(screen.queryByText('Unsaved')).not.toBeInTheDocument(),
  );
});
