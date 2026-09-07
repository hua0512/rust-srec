import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { render, screen } from '@testing-library/react';

import { ConnectionStatusIndicator } from '../connection-status-indicator';
import { useDownloadStore, type ConnectionStatus } from '@/store/downloads';

function renderIndicator(status: ConnectionStatus) {
  useDownloadStore.setState({ connectionStatus: status });

  render(
    <I18nProvider i18n={setupI18n({ locale: 'en', messages: { en: {} } })}>
      <ConnectionStatusIndicator />
    </I18nProvider>,
  );
}

describe('connection status indicator', () => {
  it('reports the connection as a status rather than a control', () => {
    renderIndicator('connected');

    expect(
      screen.getByRole('status', { name: 'Connection status: Connected' }),
    ).toBeInTheDocument();
    expect(screen.queryByRole('button')).not.toBeInTheDocument();
  });

  it('names the status it is currently showing', () => {
    renderIndicator('error');

    expect(
      screen.getByRole('status', {
        name: 'Connection status: Connection Error',
      }),
    ).toBeInTheDocument();
  });
});
