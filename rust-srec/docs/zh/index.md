---
layout: home

hero:
  name: Rust-srec
  text: 多平台自动直播录制系统
  tagline: 在一个自托管服务中监控直播间、录制视频与弹幕，并运行可重复的后处理工作流。
  actions:
    - theme: brand
      text: 快速开始
      link: /zh/getting-started/
    - theme: alt
      text: 生产部署
      link: /zh/operations/production
    - theme: alt
      text: GitHub
      link: https://github.com/hua0512/rust-srec
  image:
    src: /stream-rec-orange.svg
    alt: Rust-srec

features:
  - icon: 📝
    title: 更新日志
    details: 查看版本更新、行为变更、兼容性说明与升级建议
  - icon: 🎬
    title: 多平台支持
    details: 支持 Bilibili、抖音、斗鱼、虎牙、Twitch、TikTok 等平台
  - icon: ⚡
    title: 资源可控
    details: 可按主机容量限制任务与下载并发数
  - icon: 🔄
    title: DAG 管道
    details: 基于有向无环图的后处理工作流
  - icon: 🎯
    title: 灵活配置
    details: 4 层配置层级，精细化控制
  - icon: 💬
    title: 弹幕支持
    details: 同步录制弹幕
  - icon: 🐳
    title: 容器化部署
    details: 为单机部署提供持久化卷、健康检查与日志轮转
  - icon: 📅
    title: 自定义录制计划
    details: 定时录制，自动管理文件名，便于组织归档
  - icon: 🔧
    title: FLV 修复
    details: 自动修复损坏的 FLV 流，包括时间戳校正和元数据修复
---

<div class="showcase">

## 界面预览

  <figure>
    <img class="showcase-light" src="/images/showcase/zh-CN/dashboard-light.webp" alt="仪表盘：正在录制的直播与处理任务统计" loading="lazy" />
    <img class="showcase-dark" src="/images/showcase/zh-CN/dashboard-dark.webp" alt="仪表盘：正在录制的直播与处理任务统计" loading="lazy" />
    <figcaption>仪表盘：系统状态、处理任务与正在进行的录制</figcaption>
  </figure>
  <figure>
    <img class="showcase-light" src="/images/showcase/zh-CN/streamers-light.webp" alt="主播列表" loading="lazy" />
    <img class="showcase-dark" src="/images/showcase/zh-CN/streamers-dark.webp" alt="主播列表" loading="lazy" />
    <figcaption>所有频道一览，直播中的会显示下载速度和文件大小</figcaption>
  </figure>
  <figure>
    <img class="showcase-light" src="/images/showcase/zh-CN/sessions-light.webp" alt="场次列表" loading="lazy" />
    <img class="showcase-dark" src="/images/showcase/zh-CN/sessions-dark.webp" alt="场次列表" loading="lazy" />
    <figcaption>已录制的场次，附带标题、时长与大小</figcaption>
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
</div>
