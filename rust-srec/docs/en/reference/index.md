<script setup>
const items = [
  {
    title: 'Settings',
    description: 'Complete lookup for global, platform, template, retention, danmu, and proxy settings.',
    link: '/en/reference/settings',
  },
  {
    title: 'Environment Variables',
    description: 'Configure ports, paths, auth, shutdown deadlines, CORS, and logging via environment variables.',
    link: '/en/reference/environment',
  },
  {
    title: 'Filename Templates',
    description: 'Placeholders, date formatting, and path sanitization rules for recording output files.',
    link: '/en/reference/filenames',
  },
  {
    title: 'Configuration Overrides',
    description: 'JSON override schemas for platform options, credential selection, and proxy routing.',
    link: '/en/reference/configuration-overrides',
  },
  {
    title: 'Processors',
    description: 'Options for remux, transcode, thumbnail, rclone, Baidu Netdisk, archive, and execute steps.',
    link: '/en/reference/processors',
  },
  {
    title: 'Workflow Definitions',
    description: 'DAG step dependencies, retries, timeouts, and execution semantics for custom workflows.',
    link: '/en/reference/workflows',
  },
  {
    title: 'REST API',
    description: 'Authentication, route groups, search filtering, error formats, and OpenAPI access.',
    link: '/en/api/',
  },
  {
    title: 'API Keys & MCP',
    description: 'Create scoped API keys and connect AI assistants to the built-in MCP server.',
    link: '/en/api/api-keys-mcp',
  },
  {
    title: 'Platforms',
    description: 'Per-platform capabilities, login options, batch detection, and extraction settings.',
    link: '/en/platforms/',
  },
]
</script>

# Reference

Exact field specifications, processor options, platform capabilities, and API documentation.

<DocCardGrid :items="items" />
