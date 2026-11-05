import { screen } from '@testing-library/react';

import { ConnectionStatusIndicator } from '../connection-status-indicator';
import { useDownloadStore, type ConnectionStatus } from '@/store/downloads';
import { renderWithProviders } from '@/test/render';

function renderIndicator(status: ConnectionStatus) {
  useDownloadStore.setState({ connectionStatus: status });

  renderWithProviders(<ConnectionStatusIndicator />);
}

describe('connection status indicator', () => {
  it('reports the connection as a status rather than a control', () => {
    renderIndicator('connected');

    expect(
      screen.getByRole('status', { name: 'Connection status: Connected' }),
    ).toBeInTheDocument();
    expect(screen.queryByRole('button')).not.toBeInTheDocument();
  });
});
