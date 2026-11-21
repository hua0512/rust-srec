<script setup>
const items = [
  {
    title: '系统概览',
    description: '了解开播监测、录制引擎、弹幕采集与后处理工作流如何协同工作。',
    link: '/zh/concepts/architecture',
  },
  {
    title: '模板与继承',
    description: '通过四层配置继承、多账号池与已保存的代理高效复用频道设置。',
    link: '/zh/concepts/configuration',
  },
  {
    title: '录制时间安排',
    description: '使用时间窗口、标题关键词与分区过滤器精确控制何时录制直播。',
    link: '/zh/guides/schedules',
  },
  {
    title: '录制引擎',
    description: '对比 Mesio、FFmpeg 与 Streamlink，配置无损分段与直播流修复选项。',
    link: '/zh/concepts/engines',
  },
  {
    title: '创建工作流',
    description: '搭建可视化 DAG 后处理流水线，自动转封装、截封面、上传网盘并清理本地文件。',
    link: '/zh/concepts/pipeline',
  },
  {
    title: '通知',
    description: '通过 Telegram、Discord、Gotify、邮件、Webhook 或浏览器推送接收开播与告警通知。',
    link: '/zh/concepts/notifications',
  },
  {
    title: '弹幕与统计',
    description: '随视频同步录制弹幕、在播放器中叠加回放，并查看每场直播的互动统计。',
    link: '/zh/guides/danmu',
  },
]
</script>

# 使用指南

按任务场景组织的操作指南，帮助你管理频道、定制录制行为并搭建自动化后处理。

<DocCardGrid :items="items" />
