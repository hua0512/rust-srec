<script setup>
const items = [
  {
    title: 'Development Environment',
    description: 'Set up the Rust toolchain, Node.js, pnpm, and local test commands for contributing.',
    link: '/en/development/setup',
  },
  {
    title: 'Runtime Architecture',
    description: 'Service lifecycle, Tokio task boundaries, scheduler state, and backend Rust interfaces.',
    link: '/en/development/architecture',
  },
  {
    title: 'Configuration Resolution',
    description: 'How the 4-layer config cascade merges, caches, and validates overrides at runtime.',
    link: '/en/development/configuration',
  },
  {
    title: 'Pipeline Contracts',
    description: 'Job queue scheduling, processor result contracts, output staging, and restart recovery.',
    link: '/en/development/pipeline',
  },
  {
    title: 'Recording Engine Internals',
    description: 'Downloader traits, process supervision, cooperative shutdown, and segment reporting.',
    link: '/en/development/engines',
  },
  {
    title: 'Mesio Engine',
    description: 'Inside the native Rust FLV, HLS, and DASH downloader and real-time stream repair pipeline.',
    link: '/en/concepts/mesio',
  },
  {
    title: 'Notification Internals',
    description: 'Parallel channel delivery, queue limits, localization, and backend notification traits.',
    link: '/en/development/notifications',
  },
  {
    title: 'Persistence Contracts',
    description: 'SQLite pool budgeting, schema migrations, transaction boundaries, and repository writes.',
    link: '/en/development/persistence',
  },
  {
    title: 'Monitoring Implementation',
    description: 'Health probes, filesystem sampling, GPU diagnostics, and log archive streaming.',
    link: '/en/development/monitoring',
  },
]
</script>

# Development

Implementation contracts for contributors changing the recorder. Set up your [development environment](/en/development/setup) first, then start with the runtime architecture and follow the subsystem you are modifying.

<DocCardGrid :items="items" />
