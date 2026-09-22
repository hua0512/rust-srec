import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from '@testing-library/react';
import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { PlayerCard } from '../player-card';
import { usePlayerPlayback } from '../use-player-playback';

vi.mock('../use-player-playback', () => ({ usePlayerPlayback: vi.fn() }));
const hook = vi.mocked(usePlayerPlayback);
const reload = vi.fn();
const playback: ReturnType<typeof usePlayerPlayback> = {
  containerRef: { current: null },
  error: null,
  loading: false,
  reload,
  status: 'playing',
  connection: 'direct',
  statistics: null,
};
const i18n = setupI18n({ locale: 'en', messages: { en: {} } });

function player(props: Partial<React.ComponentProps<typeof PlayerCard>> = {}) {
  return (
    <I18nProvider i18n={i18n}>
      <PlayerCard
        url="https://media.example/live.m3u8"
        sourceUrl="https://source.example/channel?token=secret"
        title="Evening stream"
        creator="Creator"
        quality="1080p"
        {...props}
      />
    </I18nProvider>
  );
}

beforeEach(() => {
  reload.mockReset();
  hook.mockReturnValue(playback);
});

it('shows identity, status, quality and effective connection without URL credentials', () => {
  render(player());
  expect(screen.getByText('Evening stream')).toBeInTheDocument();
  expect(screen.getByText(/Creator/)).toHaveTextContent(
    'source.example/channel',
  );
  expect(screen.getByRole('status')).toHaveTextContent('Playing');
  expect(screen.getByText('1080p')).toBeInTheDocument();
  expect(document.body.textContent).not.toContain('secret');
});

it('offers retry, proxy and source selection after a playback error', () => {
  hook.mockReturnValue({ ...playback, error: 'network', status: 'error' });
  render(player({ settingsContent: <div>Source selector</div> }));
  expect(screen.getByRole('alert')).toHaveTextContent('Check the connection');
  fireEvent.click(screen.getByRole('button', { name: 'Retry' }));
  expect(reload).toHaveBeenCalledOnce();
  fireEvent.click(screen.getByRole('button', { name: 'Try server proxy' }));
  expect(hook).toHaveBeenLastCalledWith(
    expect.objectContaining({ connectionMode: 'proxy' }),
  );
  fireEvent.click(screen.getByRole('button', { name: 'Change source' }));
  expect(screen.getByText('Source selector')).toBeInTheDocument();
});

it('disables direct playback for sources needing request headers', () => {
  render(player({ headers: { Referer: 'https://source.example' } }));
  fireEvent.click(screen.getByRole('button', { name: 'Player settings' }));
  expect(screen.getByText(/Direct is unavailable/)).toBeInTheDocument();
  fireEvent.keyDown(screen.getByRole('combobox'), { key: 'ArrowDown' });
  expect(screen.getByRole('option', { name: 'Direct' })).toHaveAttribute(
    'data-disabled',
  );
});

it('shows refresh progress and a recoverable error when refreshing fails', async () => {
  let rejectRefresh: (error: Error) => void = () => {};
  const refresh = vi.fn(
    () =>
      new Promise<void>((_resolve, reject) => {
        rejectRefresh = reject;
      }),
  );
  render(player({ onRefreshSource: refresh }));
  fireEvent.click(screen.getByRole('button', { name: 'Player settings' }));
  fireEvent.click(screen.getByRole('button', { name: 'Refresh stream URL' }));
  expect(screen.getByRole('status')).toHaveTextContent('Resolving stream');
  expect(
    screen.getByRole('button', { name: 'Refresh stream URL' }),
  ).toBeDisabled();
  await act(async () => rejectRefresh(new Error('secret')));
  await waitFor(() =>
    expect(screen.getByRole('alert')).toHaveTextContent(
      'could not be refreshed',
    ),
  );
  expect(screen.getByRole('alert')).not.toHaveTextContent('secret');
});

it('separates source metadata from measured playback details and enables sampling on demand', () => {
  hook.mockReturnValue({
    ...playback,
    statistics: {
      width: 1280,
      height: 720,
      bufferSeconds: 3.5,
      droppedFrames: 0,
      totalFrames: 240,
      liveEdgeDistanceSeconds: 5.2,
    },
  });
  render(
    player({
      isLive: true,
      sourceDetails: { codec: 'h264', fps: 60, bitrate: 4_000_000 },
    }),
  );
  expect(hook).toHaveBeenLastCalledWith(
    expect.objectContaining({ detailsEnabled: false }),
  );
  fireEvent.click(screen.getByRole('button', { name: 'Playback details' }));
  expect(hook).toHaveBeenLastCalledWith(
    expect.objectContaining({ detailsEnabled: true }),
  );
  expect(screen.getByText('Reported by source')).toBeInTheDocument();
  expect(screen.getByText('Measured by player')).toBeInTheDocument();
  expect(screen.getByText('h264')).toBeInTheDocument();
  expect(screen.getByText('60 fps')).toBeInTheDocument();
  expect(screen.getByText('1280 \u00d7 720')).toBeInTheDocument();
  expect(screen.getByText('0 / 240')).toBeInTheDocument();
  expect(screen.getByText('5.2 s')).toBeInTheDocument();
});
