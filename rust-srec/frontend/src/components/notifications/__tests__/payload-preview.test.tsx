import { I18nProvider } from '@lingui/react';
import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import { PayloadPreview } from '../events/payload-preview';
import { testI18n } from '@/test/render';

describe('PayloadPreview', () => {
  it('unwraps credential events nested under `event`', () => {
    const i18n = testI18n();
    render(
      <I18nProvider i18n={i18n}>
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
        />
      </I18nProvider>,
    );

    expect(screen.getByText('bilibili')).toBeInTheDocument();
    expect(screen.getByText('cookie expired')).toBeInTheDocument();
  });
});
