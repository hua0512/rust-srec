<script setup>
const items = [
  {
    title: 'Installation',
    description: 'Install the desktop app, download prebuilt binaries, set up systemd, or build from source.',
    link: '/en/getting-started/installation',
  },
  {
    title: 'Docker Deployment',
    description: 'Run the backend and web interface with Docker Compose, persistent volumes, and optional GPU support.',
    link: '/en/getting-started/docker',
  },
  {
    title: 'First Recording',
    description: 'Sign in, add your first streamer, verify live detection, and inspect recorded segments.',
    link: '/en/getting-started/first-recording',
  },
  {
    title: 'Basic Configuration',
    description: 'Configure output directories, naming templates, danmu capture, and starter post-processing.',
    link: '/en/getting-started/configuration',
  },
  {
    title: 'FAQ',
    description: 'Answers to common setup, platform login, engine, and troubleshooting questions.',
    link: '/en/getting-started/faq',
  },
]
</script>

# Getting Started

**Rust-Srec** is a self-hosted automatic livestream recorder built with Rust. It supports 14 built-in platforms and brings together automatic live detection, stream repair, danmaku collection, post-processing workflows, and remote management in a web interface and REST API.

Choose a path below to get up and running:

<DocCardGrid :items="items" />

## System Requirements

- **OS**: Linux, macOS, or Windows (desktop app, standalone binaries, or Docker)
- **Database**: SQLite (bundled — no external database server required)
- **Rust**: 1.95 or newer (only needed when building from source)
