---
layout: home

hero:
  name: Rust-srec
  text: Automated Multi-Platform Stream Recorder
  tagline: Monitor live channels, record video and chat, and run repeatable post-processing workflows from one self-hosted service.
  actions:
    - theme: brand
      text: Get Started
      link: /en/getting-started/
    - theme: alt
      text: Production Guide
      link: /en/operations/production
    - theme: alt
      text: View on GitHub
      link: https://github.com/hua0512/rust-srec
  image:
    src: /stream-rec-orange.svg
    alt: Rust-srec

features:
  - icon: 📝
    title: Release Notes
    details: Track version updates, behavior changes, compatibility notes, and upgrade guidance
  - icon: 🎬
    title: Multi-Platform Support
    details: Support for Bilibili, Douyin, Douyu, Huya, Twitch, TikTok, and more
  - icon: ⚡
    title: Controlled Resource Use
    details: Set task and download concurrency limits to match the capacity of your host
  - icon: 🔄
    title: DAG Pipeline
    details: Post-processing with directed acyclic graph workflows
  - icon: 🎯
    title: Flexible Configuration
    details: 4-layer config hierarchy for fine-grained control
  - icon: 💬
    title: Danmaku Support
    details: Capture live chat/danmaku alongside video streams
  - icon: 🐳
    title: Container Ready
    details: Persistent volumes, health checks, and log rotation for single-node deployments
  - icon: 📅
    title: Customizable Recording Schedules
    details: Schedule recording times and automatically manage file names for easy organization
  - icon: 🔧
    title: FLV Fix
    details: Automatically repair broken FLV streams with timestamp correction and metadata fixing
---

<div class="showcase">

## Screenshots

  <figure>
    <img class="showcase-light" src="/images/showcase/en/dashboard-light.webp" alt="Dashboard with live recordings and pipeline statistics" loading="lazy" />
    <img class="showcase-dark" src="/images/showcase/en/dashboard-dark.webp" alt="Dashboard with live recordings and pipeline statistics" loading="lazy" />
    <figcaption>The dashboard: system health, processing jobs and active recordings</figcaption>
  </figure>
  <figure>
    <img class="showcase-light" src="/images/showcase/en/streamers-light.webp" alt="Streamer list" loading="lazy" />
    <img class="showcase-dark" src="/images/showcase/en/streamers-dark.webp" alt="Streamer list" loading="lazy" />
    <figcaption>Every channel at a glance, live ones with download speed and size</figcaption>
  </figure>
  <figure>
    <img class="showcase-light" src="/images/showcase/en/sessions-light.webp" alt="Session list" loading="lazy" />
    <img class="showcase-dark" src="/images/showcase/en/sessions-dark.webp" alt="Session list" loading="lazy" />
    <figcaption>Recorded sessions with titles, duration and size</figcaption>
  </figure>
  <figure>
    <img class="showcase-light" src="/images/showcase/en/danmu-stats-light.webp" alt="Chat statistics for a session" loading="lazy" />
    <img class="showcase-dark" src="/images/showcase/en/danmu-stats-dark.webp" alt="Chat statistics for a session" loading="lazy" />
    <figcaption>Chat activity, top chatters and top words for each session</figcaption>
  </figure>
  <figure>
    <img class="showcase-light" src="/images/showcase/en/workflow-light.webp" alt="Workflow editor in graph view" loading="lazy" />
    <img class="showcase-dark" src="/images/showcase/en/workflow-dark.webp" alt="Workflow editor in graph view" loading="lazy" />
    <figcaption>Build post-processing workflows as a graph</figcaption>
  </figure>
  <figure>
    <img class="showcase-light" src="/images/showcase/en/jobs-light.webp" alt="Pipeline jobs list" loading="lazy" />
    <img class="showcase-dark" src="/images/showcase/en/jobs-dark.webp" alt="Pipeline jobs list" loading="lazy" />
    <figcaption>Follow every processing job as it runs</figcaption>
  </figure>
</div>
