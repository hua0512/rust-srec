import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { render, screen } from '@testing-library/react';
import { useForm } from 'react-hook-form';

import { Form } from '@/components/ui/form';
import {
  SharedConfigEditor,
  type SharedConfigPaths,
} from '../shared-config-editor';

type Values = Record<string, unknown>;

const PATHS: SharedConfigPaths<Values> = {
  streamSelection: 'stream_selection_config',
  cookies: 'cookies',
  proxy: 'proxy_config',
  retryPolicy: 'download_retry_policy',
  output: '',
  limits: '',
  danmu: '',
  pipeline: 'pipeline',
};

function renderDanmuTab(paths: SharedConfigPaths<Values>) {
  function Harness() {
    const form = useForm<Values>({ defaultValues: {} });
    return (
      <Form {...form}>
        <SharedConfigEditor
          form={form}
          paths={paths}
          availableTabs={['danmu']}
          defaultTab="danmu"
        />
      </Form>
    );
  }
  render(
    <I18nProvider i18n={setupI18n({ locale: 'en', messages: { en: {} } })}>
      <Harness />
    </I18nProvider>,
  );
}

describe('SharedConfigEditor danmu tab', () => {
  it('shows the statistics settings for a layer that resolves them', () => {
    renderDanmuTab({ ...PATHS, danmuStatistics: '' });

    expect(screen.getByText('Danmu Statistics')).toBeInTheDocument();
  });

  it('leaves the statistics settings out when the layer has no path for them', () => {
    renderDanmuTab(PATHS);

    expect(screen.queryByText('Danmu Statistics')).not.toBeInTheDocument();
  });
});
