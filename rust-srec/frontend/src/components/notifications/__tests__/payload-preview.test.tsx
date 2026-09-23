import { setupI18n } from '@lingui/core';
import { msg } from '@lingui/core/macro';
import { I18nProvider } from '@lingui/react';
import { render, screen } from '@testing-library/react';
import type { ReactNode } from 'react';
import { describe, expect, it } from 'vitest';

import { PayloadPreview } from '../events/payload-preview';

const payload = JSON.stringify({
  DownloadCompleted: {
    streamer_name: 'Streamer One',
    duration_secs: 3725,
    file_size_bytes: 1572864,
  },
});

// Descriptors built here carry the same generated ids as the ones inside the
// component, so a catalog keyed by them proves the labels go through `i18n`.
const duration = msg`Duration`;
const size = msg`Size`;

function renderWith(i18n: ReturnType<typeof setupI18n>, children: ReactNode) {
  return render(<I18nProvider i18n={i18n}>{children}</I18nProvider>);
}

describe('PayloadPreview', () => {
  it('formats duration and size with the shared helpers', () => {
    const i18n = setupI18n({ locale: 'en', messages: { en: {} } });
    renderWith(i18n, <PayloadPreview payload={payload} />);

    expect(screen.getByText('1h 2m')).toBeInTheDocument();
    expect(screen.getByText('1.5 MB')).toBeInTheDocument();
    expect(screen.getByText('Duration:')).toBeInTheDocument();
    expect(screen.getByText('Size:')).toBeInTheDocument();
  });

  it('renders field labels from the active catalog', () => {
    const i18n = setupI18n({
      locale: 'xx',
      messages: {
        xx: {
          [String(duration.id)]: 'DURACION',
          [String(size.id)]: 'TAMANO',
        },
      },
    });
    renderWith(i18n, <PayloadPreview payload={payload} />);

    expect(screen.getByText('DURACION:')).toBeInTheDocument();
    expect(screen.getByText('TAMANO:')).toBeInTheDocument();
  });

  it('reads any event variant, not only a hand-listed set', () => {
    const i18n = setupI18n({ locale: 'en', messages: { en: {} } });
    renderWith(
      i18n,
      <PayloadPreview
        payload={JSON.stringify({
          OutputPathInaccessible: {
            path: '/rec',
            error_kind: 'permission_denied',
            timestamp: '2026-01-01T00:00:00Z',
          },
        })}
      />,
    );

    expect(screen.getByText('/rec')).toBeInTheDocument();
    expect(screen.getByText('permission_denied')).toBeInTheDocument();
  });

  it('falls back to the message when an event has no error field', () => {
    const i18n = setupI18n({ locale: 'en', messages: { en: {} } });
    renderWith(
      i18n,
      <PayloadPreview
        payload={JSON.stringify({
          BaiduPcsReloginFailed: {
            config_dir: '/data/baidupcs',
            message: 'login expired',
            timestamp: '2026-01-01T00:00:00Z',
          },
        })}
      />,
    );

    expect(screen.getByText('login expired')).toBeInTheDocument();
  });

  it('unwraps credential events nested under `event`', () => {
    const i18n = setupI18n({ locale: 'en', messages: { en: {} } });
    renderWith(
      i18n,
      <PayloadPreview
        payload={JSON.stringify({
          Credential: {
            event: {
              type: 'refresh_failed',
              platform: 'bilibili',
              error: 'cookie expired',
            },
          },
        })}
      />,
    );

    expect(screen.getByText('bilibili')).toBeInTheDocument();
    expect(screen.getByText('cookie expired')).toBeInTheDocument();
  });
});
