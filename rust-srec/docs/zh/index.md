---
layout: home

hero:
  name: Rust-srec
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
  image:
    src: /stream-rec-orange.svg
    alt: Rust-srec

features:
  - icon: 🔴
    title: 全自动录制
    details: 添加一次主播，开播立刻开录。可以全天候守候，也可以只在你设定的时间段录制。
    link: /zh/getting-started/first-recording
  - icon: 💬
    title: 弹幕一起留下
    details: 弹幕与视频一同保存，播放时可叠加在画面上回放，每场直播还有弹幕统计。
    link: /zh/guides/danmu
  - icon: 🩹
    title: 录像拿来就能看
    details: 直播流卡顿、时长丢失等问题会在录制时自动修复，录出来的文件在任何播放器里都能正常播放和拖动。
    link: /zh/concepts/engines
  - icon: ☁️
    title: 录完自动收尾
    details: 自动转码、生成封面、上传到百度网盘、Google Drive 或 OneDrive 并清理本地文件，工作流可视化搭建。
    link: /zh/concepts/pipeline
  - icon: 🔔
    title: 动态及时通知
    details: 开播、录制结束或需要处理的问题，都能通过 Telegram、Gotify、邮件或 Webhook 通知你。
    link: /zh/concepts/notifications
  - icon: 🖥️
    title: 想在哪儿跑就在哪儿跑
    details: Windows、macOS、Linux 桌面版，或用 Docker 部署到家用服务器、NAS，在任意浏览器里管理，手机也行。
    link: /zh/getting-started/installation
---

<div class="showcase">

## 界面预览

  <figure>
    <img class="showcase-light" src="/images/showcase/zh-CN/dashboard-light.webp" alt="仪表盘：正在录制的直播与处理任务统计" loading="lazy" />
    <img class="showcase-dark" src="/images/showcase/zh-CN/dashboard-dark.webp" alt="仪表盘：正在录制的直播与处理任务统计" loading="lazy" />
    <figcaption>仪表盘：系统状态、处理任务与正在进行的录制</figcaption>
  </figure>
  <figure>
    <img class="showcase-light" src="/images/showcase/zh-CN/player-light.webp" alt="回放录像，弹幕在画面上滚动" loading="lazy" />
    <img class="showcase-dark" src="/images/showcase/zh-CN/player-dark.webp" alt="回放录像，弹幕在画面上滚动" loading="lazy" />
    <figcaption>回放录像，弹幕同步叠加在画面上</figcaption>
  </figure>
  <figure>
    <img class="showcase-light" src="/images/showcase/zh-CN/streamers-light.webp" alt="主播列表" loading="lazy" />
    <img class="showcase-dark" src="/images/showcase/zh-CN/streamers-dark.webp" alt="主播列表" loading="lazy" />
    <figcaption>所有频道一览，直播中的会显示下载速度和文件大小</figcaption>
  </figure>
  <figure>
    <img class="showcase-light" src="/images/showcase/zh-CN/session-light.webp" alt="录制会话详情页" loading="lazy" />
    <img class="showcase-dark" src="/images/showcase/zh-CN/session-dark.webp" alt="录制会话详情页" loading="lazy" />
    <figcaption>每场录制都有可直接播放的预览</figcaption>
  </figure>
  <figure>
    <img class="showcase-light" src="/images/showcase/zh-CN/danmu-stats-light.webp" alt="场次弹幕统计" loading="lazy" />
    <img class="showcase-dark" src="/images/showcase/zh-CN/danmu-stats-dark.webp" alt="场次弹幕统计" loading="lazy" />
    <figcaption>每场直播的弹幕走势、活跃观众与高频词</figcaption>
  </figure>
  <figure>
    <img class="showcase-light" src="/images/showcase/zh-CN/workflow-light.webp" alt="工作流编辑器（图视图）" loading="lazy" />
    <img class="showcase-dark" src="/images/showcase/zh-CN/workflow-dark.webp" alt="工作流编辑器（图视图）" loading="lazy" />
    <figcaption>以图的方式编排后处理工作流</figcaption>
  </figure>
  <figure>
    <img class="showcase-light" src="/images/showcase/zh-CN/jobs-light.webp" alt="处理任务列表" loading="lazy" />
    <img class="showcase-dark" src="/images/showcase/zh-CN/jobs-dark.webp" alt="处理任务列表" loading="lazy" />
    <figcaption>实时查看每个处理任务的进度</figcaption>
  </figure>
  <figure>
    <img class="showcase-light" src="/images/showcase/zh-CN/mobile-light.webp" alt="手机上的 Web 界面" loading="lazy" />
    <img class="showcase-dark" src="/images/showcase/zh-CN/mobile-dark.webp" alt="手机上的 Web 界面" loading="lazy" />
    <figcaption>在手机上随时查看录制情况</figcaption>
  </figure>
</div>
