---
layout: home

hero:
  name: Rust-Srec
  text: 再也不错过任何一场直播
  tagline: 主播一开播就自动录制视频和弹幕，支持 B 站、抖音、虎牙、Twitch、TikTok 等平台，运行在你自己的电脑或服务器上。
  actions:
    - theme: brand
      text: 快速开始
      link: /zh/getting-started/
    - theme: alt
      text: 下载
      link: https://github.com/hua0512/rust-srec/releases/latest
    - theme: alt
      text: GitHub
      link: https://github.com/hua0512/rust-srec

features:
  - icon: '<svg xmlns="http://www.w3.org/2000/svg" width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M4.9 19.1C1 15.2 1 8.8 4.9 4.9"/><path d="M7.8 16.2c-2.3-2.3-2.3-6.1 0-8.5"/><circle cx="12" cy="12" r="2"/><path d="M16.2 7.8c2.3 2.3 2.3 6.1 0 8.5"/><path d="M19.1 4.9C23 8.8 23 15.1 19.1 19"/></svg>'
    title: 全自动录制
    details: 添加一次主播，开播立刻开录。可以全天候守候，也可以只在你设定的时间段录制。
    link: /zh/getting-started/first-recording
    linkText: 了解更多
  - icon: '<svg xmlns="http://www.w3.org/2000/svg" width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/><path d="M13 8H7"/><path d="M17 12H7"/></svg>'
    title: 弹幕一起留下
    details: 弹幕与视频一同保存，播放时可叠加在画面上回放，每场直播还有弹幕统计。
    link: /zh/guides/danmu
    linkText: 了解更多
  - icon: '<svg xmlns="http://www.w3.org/2000/svg" width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1z"/><path d="m9 12 2 2 4-4"/></svg>'
    title: 录像拿来就能看
    details: 直播流卡顿、时长丢失等问题会在录制时自动修复，录出来的文件在任何播放器里都能正常播放和拖动。
    link: /zh/concepts/engines
    linkText: 了解更多
  - icon: '<svg xmlns="http://www.w3.org/2000/svg" width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect width="8" height="8" x="3" y="3" rx="2"/><path d="M7 11v4a2 2 0 0 0 2 2h4"/><rect width="8" height="8" x="13" y="13" rx="2"/></svg>'
    title: 录完自动收尾
    details: 自动转码、生成封面、上传到百度网盘、Google Drive 或 OneDrive 并清理本地文件，工作流可视化搭建。
    link: /zh/concepts/pipeline
    linkText: 了解更多
  - icon: '<svg xmlns="http://www.w3.org/2000/svg" width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3s3-2 3-9"/><path d="M10.3 21a1.94 1.94 0 0 0 3.4 0"/></svg>'
    title: 动态及时通知
    details: 开播、录制结束或需要处理的问题，都能通过 Telegram、Gotify、邮件或 Webhook 通知你。
    link: /zh/concepts/notifications
    linkText: 了解更多
  - icon: '<svg xmlns="http://www.w3.org/2000/svg" width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M18 8V6a2 2 0 0 0-2-2H4a2 2 0 0 0-2 2v7a2 2 0 0 0 2 2h8"/><path d="M10 19v-3.96 3.15"/><path d="M7 19h5"/><rect width="6" height="10" x="16" y="12" rx="2"/></svg>'
    title: 想在哪儿跑就在哪儿跑
    details: Windows、macOS、Linux 桌面版，或用 Docker 部署到家用服务器、NAS，在任意浏览器里管理，手机也行。
    link: /zh/getting-started/installation
    linkText: 了解更多
---

<div class="vp-home-section">
  <div class="vp-home-section-header">
    <span class="vp-section-eyebrow">工作流程</span>
    <h2>从开播监测到归档回放，全程自动接管</h2>
    <p>一键了解整场直播如何经过自动监测、实时流修复、后处理工作流与多渠道推送完整留存。</p>
  </div>
  <AnimatedFlowDemo />
</div>

<div class="vp-home-section">
  <div class="vp-home-section-header">
    <span class="vp-section-eyebrow">支持平台</span>
    <h2>14+ 主流直播平台，一套工具统一录制</h2>
    <p>内置海内外主流直播平台解析器与同步弹幕引擎，无需额外拼装复杂脚本。</p>
  </div>
  <PlatformMarquee />
</div>

<div class="vp-home-section">
  <div class="vp-home-section-header">
    <span class="vp-section-eyebrow">界面预览</span>
    <h2>桌面端与网页端统一的现代化控制台</h2>
    <p>在任意浏览器或桌面应用中管理主播、带滚动弹幕回放录像、查看弹幕热力统计并可视化编排工作流。</p>
  </div>
  <HomeShowcase />
</div>

<div class="vp-home-section">
  <div class="vp-home-section-header">
    <span class="vp-section-eyebrow">版本对比</span>
    <h2>核心能力永久开源，亦可从容扩展</h2>
    <p>社区版提供全部自托管录制、弹幕回放、工作流与 API 能力，无功能锁；未来将提供面向大规模监控的可选批量增强服务。</p>
  </div>
  <TierComparison />
</div>

<div class="vp-home-section">
  <HomeBottomCta />
</div>
