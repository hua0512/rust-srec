<script setup>
const items = [
  {
    title: 'Production Deployment',
    description: 'Reference single-node topology, reverse proxy setup, and process management for always-on hosts.',
    link: '/en/operations/production',
  },
  {
    title: 'Storage & Capacity',
    description: 'Estimate disk usage, organize output roots, configure write probes, and avoid disk-full stalls.',
    link: '/en/operations/storage',
  },
  {
    title: 'Backup & Restore',
    description: 'Combine portable configuration exports with filesystem snapshots for full disaster recovery.',
    link: '/en/operations/backup-restore',
  },
  {
    title: 'Monitoring',
    description: 'Track health endpoints, application logs, SQLite memory budgets, and scheduler restart limits.',
    link: '/en/operations/monitoring',
  },
  {
    title: 'Upgrading & Rollback',
    description: 'Safely upgrade Docker, systemd, or binary deployments and roll back using pre-upgrade snapshots.',
    link: '/en/operations/upgrading',
  },
  {
    title: 'Security',
    description: 'Protect login sessions, API keys, platform cookies, reverse proxies, and host files.',
    link: '/en/operations/security',
  },
  {
    title: 'Data Governance',
    description: 'Data inventory, retention boundaries, and operator responsibilities for recorded broadcasts.',
    link: '/en/operations/data-governance',
  },
  {
    title: 'Support & Versions',
    description: 'Scope, single-node boundaries, version lifecycle, and how to report issues effectively.',
    link: '/en/operations/support',
  },
]
</script>

# Operations

Runbooks and checklists for deploying, securing, monitoring, and maintaining Rust-Srec in production.

<DocCardGrid :items="items" />
