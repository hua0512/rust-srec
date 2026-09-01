import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { render, screen } from '@testing-library/react';

import { StorageUsageCard, groupDisksByMountPoint } from './storage-usage-card';
import type { ComponentHealth } from '@/api/schemas/system';

const GB = 1024 * 1024 * 1024;

function diskComponent(
  path: string,
  mountPoint: string,
  availableGb: number,
  totalGb: number,
  status = 'healthy',
): ComponentHealth {
  const available = availableGb * GB;
  const total = totalGb * GB;
  return {
    name: `disk:${path}`,
    status,
    disk: {
      path,
      mount_point: mountPoint,
      total_bytes: total,
      available_bytes: available,
      used_bytes: total - available,
      used_percent: (1 - available / total) * 100,
    },
  };
}

describe('groupDisksByMountPoint', () => {
  it('ignores components without capacity', () => {
    expect(
      groupDisksByMountPoint([
        { name: 'database', status: 'healthy' },
        { name: 'disk:/rec', status: 'unknown' },
      ]),
    ).toEqual([]);
  });

  it('collapses paths that share a filesystem into one group', () => {
    const groups = groupDisksByMountPoint([
      diskComponent('/rec', '/', 40, 100),
      diskComponent('/var/lib/srec', '/', 40, 100),
    ]);

    expect(groups).toHaveLength(1);
    expect(groups[0].mountPoint).toBe('/');
    expect(groups[0].paths).toEqual(['/rec', '/var/lib/srec']);
  });

  it('keeps separate filesystems apart, fullest first', () => {
    const groups = groupDisksByMountPoint([
      diskComponent('/rec', '/', 80, 100),
      diskComponent('/mnt/archive', '/mnt/archive', 5, 100),
    ]);

    expect(groups.map((g) => g.mountPoint)).toEqual(['/mnt/archive', '/']);
  });

  it('reports the worst status among paths on one filesystem', () => {
    const groups = groupDisksByMountPoint([
      diskComponent('/rec', '/', 15, 100, 'degraded'),
      diskComponent('/var/lib/srec', '/', 15, 100, 'healthy'),
    ]);

    expect(groups[0].status).toBe('degraded');
  });
});

function renderCard(components: ComponentHealth[]) {
  const i18n = setupI18n({ locale: 'en', messages: { en: {} } });
  return render(
    <I18nProvider i18n={i18n}>
      <StorageUsageCard components={components} />
    </I18nProvider>,
  );
}

describe('StorageUsageCard', () => {
  it('renders nothing when no component reports capacity', () => {
    const { container } = renderCard([{ name: 'database', status: 'healthy' }]);

    expect(container).toBeEmptyDOMElement();
  });

  it('shows free space with a bar sized to the used share', () => {
    renderCard([diskComponent('/rec', '/', 40, 100)]);

    expect(screen.getByText('40 GB free')).toBeInTheDocument();
    expect(screen.getByRole('progressbar')).toHaveAttribute(
      'aria-valuenow',
      '60',
    );
  });

  it('colours the bar from the status, not a duplicated threshold', () => {
    renderCard([diskComponent('/rec', '/', 15, 100, 'degraded')]);

    const indicator = screen
      .getByRole('progressbar')
      .querySelector('[data-slot="progress-indicator"]');
    expect(indicator).toHaveClass('bg-yellow-500');
  });
});
