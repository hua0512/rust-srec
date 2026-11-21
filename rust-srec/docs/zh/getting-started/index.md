<script setup>
const items = [
  {
    title: '安装指南',
    description: '安装桌面客户端、下载预编译二进制、配置 systemd 服务或从源码构建。',
    link: '/zh/getting-started/installation',
  },
  {
    title: 'Docker 部署',
    description: '使用 Docker Compose 一键部署后端与 Web 界面，配置持久化卷与可选 GPU 加速。',
    link: '/zh/getting-started/docker',
  },
  {
    title: '首次录制',
    description: '完成首次登录、添加第一位主播、确认开播检测并查看录制产物。',
    link: '/zh/getting-started/first-recording',
  },
  {
    title: '基础配置',
    description: '设置输出目录、文件命名模板、弹幕采集以及常用后处理选项。',
    link: '/zh/getting-started/configuration',
  },
  {
    title: '常见问题',
    description: '集中解答安装部署、平台登录、录制引擎与日常排错中的常见问题。',
    link: '/zh/getting-started/faq',
  },
]
</script>

# 快速开始

**Rust-Srec** 是一个基于 Rust 构建的自托管自动直播录制器。它内置支持 14 个直播平台，将自动开播监测、实时流修复、弹幕采集、后处理工作流以及 Web 界面与 REST API 管理整合在一起。

按以下步骤快速上手：

<DocCardGrid :items="items" />

## 系统要求

- **操作系统**：Linux、macOS 或 Windows（支持桌面客户端、独立二进制或 Docker）
- **数据库**：SQLite（内置，无需额外搭建数据库服务）
- **Rust**：1.95 或更高版本（仅从源码构建时需要）
