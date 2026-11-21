<script setup>
const items = [
  {
    title: 'System Overview',
    description: 'See how live monitoring, recording engines, danmu capture, and post-processing fit together.',
    link: '/en/concepts/architecture',
  },
  {
    title: 'Templates & Inheritance',
    description: 'Reuse settings across channels with the 4-layer cascade, account pools, and saved proxies.',
    link: '/en/concepts/configuration',
  },
  {
    title: 'Recording Schedules',
    description: 'Control when streams are recorded using time windows, title keywords, and category filters.',
    link: '/en/guides/schedules',
  },
  {
    title: 'Recording Engines',
    description: 'Compare Mesio, FFmpeg, and Streamlink, configure lossless splitting, and tune stream repair.',
    link: '/en/concepts/engines',
  },
  {
    title: 'Create a Workflow',
    description: 'Build visual DAG pipelines to remux, generate thumbnails, upload to cloud storage, and clean up.',
    link: '/en/concepts/pipeline',
  },
  {
    title: 'Notifications',
    description: 'Send live alerts, errors, and status updates via Telegram, Discord, Gotify, email, webhooks, or Web Push.',
    link: '/en/concepts/notifications',
  },
  {
    title: 'Danmu & Statistics',
    description: 'Capture live chat alongside video, replay it over recordings, and explore session analytics.',
    link: '/en/guides/danmu',
  },
]
</script>

# Guides

Task-based guides for organizing channels, configuring recording behavior, and automating post-processing.

<DocCardGrid :items="items" />
