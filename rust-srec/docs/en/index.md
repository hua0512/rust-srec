---
layout: home

hero:
  name: Rust-Srec
  text: Never miss a stream again
  tagline: Records your favorite streamers and their chat the moment they go live, on Bilibili, Douyin, Huya, Twitch, TikTok and more, on your own computer or server.
  actions:
    - theme: brand
      text: Get Started
      link: /en/getting-started/
    - theme: alt
      text: Download
      link: https://github.com/hua0512/rust-srec/releases/latest
    - theme: alt
      text: View on GitHub
      link: https://github.com/hua0512/rust-srec

features:
  - icon: '<svg xmlns="http://www.w3.org/2000/svg" width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M4.9 19.1C1 15.2 1 8.8 4.9 4.9"/><path d="M7.8 16.2c-2.3-2.3-2.3-6.1 0-8.5"/><circle cx="12" cy="12" r="2"/><path d="M16.2 7.8c2.3 2.3 2.3 6.1 0 8.5"/><path d="M19.1 4.9C23 8.8 23 15.1 19.1 19"/></svg>'
    title: Records on its own
    details: Add a channel once. Recording starts as soon as the stream goes live, around the clock or only in the hours you choose.
    link: /en/getting-started/first-recording
    linkText: Learn more
  - icon: '<svg xmlns="http://www.w3.org/2000/svg" width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/><path d="M13 8H7"/><path d="M17 12H7"/></svg>'
    title: Keeps the chat
    details: Danmaku and live chat are saved beside the video, replayed over it in the player, and summed up for every session.
    link: /en/guides/danmu
    linkText: Learn more
  - icon: '<svg xmlns="http://www.w3.org/2000/svg" width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1z"/><path d="m9 12 2 2 4-4"/></svg>'
    title: Recordings that play
    details: Streams that stutter or lose their length are repaired while they record, so files open and seek properly in any player.
    link: /en/concepts/engines
    linkText: Learn more
  - icon: '<svg xmlns="http://www.w3.org/2000/svg" width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect width="8" height="8" x="3" y="3" rx="2"/><path d="M7 11v4a2 2 0 0 0 2 2h4"/><rect width="8" height="8" x="13" y="13" rx="2"/></svg>'
    title: Finishes the job for you
    details: Convert, make thumbnails, upload to Google Drive, OneDrive or Baidu Netdisk and tidy up, in a workflow you build visually.
    link: /en/concepts/pipeline
    linkText: Learn more
  - icon: '<svg xmlns="http://www.w3.org/2000/svg" width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3s3-2 3-9"/><path d="M10.3 21a1.94 1.94 0 0 0 3.4 0"/></svg>'
    title: Keeps you posted
    details: Get a Telegram, Gotify, email or webhook message when a stream starts, a recording ends or something needs attention.
    link: /en/concepts/notifications
    linkText: Learn more
  - icon: '<svg xmlns="http://www.w3.org/2000/svg" width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M18 8V6a2 2 0 0 0-2-2H4a2 2 0 0 0-2 2v7a2 2 0 0 0 2 2h8"/><path d="M10 19v-3.96 3.15"/><path d="M7 19h5"/><rect width="6" height="10" x="16" y="12" rx="2"/></svg>'
    title: Runs where you want
    details: A desktop app for Windows, macOS and Linux, or Docker on a home server or NAS, managed from any browser, even on your phone.
    link: /en/getting-started/installation
    linkText: Learn more
---

<div class="vp-home-section">
  <div class="vp-home-section-header">
    <span class="vp-section-eyebrow">How It Works</span>
    <h2>Automated from live signal to archived replay</h2>
    <p>Follow every step of the recording lifecycle — detection, live stream repair, post-processing workflows, and instant alerts.</p>
  </div>
  <AnimatedFlowDemo />
</div>

<div class="vp-home-section">
  <div class="vp-home-section-header">
    <span class="vp-section-eyebrow">Supported Platforms</span>
    <h2>14+ streaming platforms, one unified recorder</h2>
    <p>Built-in extractors and synchronized Danmu providers across major global and Chinese live streaming services.</p>
  </div>
  <PlatformMarquee />
</div>

<div class="vp-home-section">
  <div class="vp-home-section-header">
    <span class="vp-section-eyebrow">Web &amp; Desktop Interface</span>
    <h2>A studio-grade console for every stream</h2>
    <p>Manage channels, replay recordings with scrolling chat, inspect Danmu analytics, and design visual workflows from any browser.</p>
  </div>
  <HomeShowcase />
</div>

<div class="vp-home-section">
  <div class="vp-home-section-header">
    <span class="vp-section-eyebrow">Editions</span>
    <h2>Open source at the core, ready to scale</h2>
    <p>Every recording, Danmu, workflow, and API feature is free forever in the Community edition, with optional large-scale batch tooling on the roadmap.</p>
  </div>
  <TierComparison />
</div>

<div class="vp-home-section">
  <HomeBottomCta />
</div>
