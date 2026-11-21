import { groupDisksByMountPoint } from './storage-usage-card';
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
