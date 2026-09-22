import { defineConfig } from "vitepress";
import { vitepressMermaidPreview } from "vitepress-mermaid-preview";

export default defineConfig({
  title: "Rust-Srec",
  description: "Automatic Online Streaming Recorder",
  lastUpdated: true,
  srcExclude: ["AGENTS.md", "release-notes.md", "release-notes-body.md"],

  // Ignore dead links for external URLs and runtime-generated paths
  ignoreDeadLinks: [
    /^\/api\/docs/,
    /^http:\/\/localhost/,
    "/docker-compose.example.yml",
    "/docker-compose.gpu.yml",
    "/env.example",
    "/env.zh.example",
  ],

  markdown: {
    config: (md) => {
      md.use(vitepressMermaidPreview);
    },
  },

  head: [
    ["link", { rel: "icon", type: "image/svg+xml", href: "/stream-rec.svg" }],
    ["meta", { name: "theme-color", content: "#e44d26" }],
  ],

  locales: {
    en: {
      label: "English",
      lang: "en",
      link: "/en/",
      themeConfig: {
        editLink: {
          pattern:
            "https://github.com/hua0512/rust-srec/edit/main/rust-srec/docs/:path",
          text: "Edit this page on GitHub",
        },
        lastUpdated: {
          text: "Last Updated",
          formatOptions: {
            dateStyle: "medium",
            timeStyle: "short",
          },
        },
        outline: { level: [2, 3], label: "On this page" },
        docFooter: { prev: "Previous page", next: "Next page" },
        darkModeSwitchLabel: "Appearance",
        lightModeSwitchTitle: "Switch to light theme",
        darkModeSwitchTitle: "Switch to dark theme",
        sidebarMenuLabel: "Menu",
        returnToTopLabel: "Return to top",
        langMenuLabel: "Change language",
        skipToContentLabel: "Skip to content",
        footer: {
          message: "Released under the MIT License.",
          copyright: "Copyright © 2024-present hua0512",
        },
        nav: [
          { text: "Getting Started", link: "/en/getting-started/" },
          { text: "Guides", link: "/en/guides/" },
          { text: "Reference", link: "/en/reference/" },
          { text: "Operations", link: "/en/operations/production" },
          { text: "Release Notes", link: "/en/release-notes/" },
          { text: "Donate", link: "/en/donate" },
          {
            text: "v0.5.1",
            items: [
              { text: "Documentation versions", link: "/en/operations/support" },
              { text: "v0.5.1 release notes", link: "/en/release-notes/v0.5.1" },
            ],
          },
        ],
        sidebar: {
          "/en/": [
            {
              text: "Getting Started",
              items: [
                { text: "Introduction", link: "/en/getting-started/" },
                { text: "Installation", link: "/en/getting-started/installation" },
                { text: "Docker", link: "/en/getting-started/docker" },
                { text: "First Recording", link: "/en/getting-started/first-recording" },
                { text: "Basic Configuration", link: "/en/getting-started/configuration" },
                { text: "FAQ", link: "/en/getting-started/faq" },
              ],
            },
            {
              text: "Guides",
              items: [
                { text: "System Overview", link: "/en/concepts/architecture" },
                { text: "Templates & Inheritance", link: "/en/concepts/configuration" },
                { text: "Recording Schedules", link: "/en/guides/schedules" },
                { text: "Recording Engines", link: "/en/concepts/engines" },
                { text: "Create a Workflow", link: "/en/concepts/pipeline" },
                { text: "Notifications", link: "/en/concepts/notifications" },
                { text: "Danmu & Statistics", link: "/en/guides/danmu" },
              ],
              link: "/en/guides/",
            },
            {
              text: "Reference",
              items: [
                { text: "Settings", link: "/en/reference/settings" },
                { text: "Environment Variables", link: "/en/reference/environment" },
                { text: "Filename Templates", link: "/en/reference/filenames" },
                { text: "Configuration Overrides", link: "/en/reference/configuration-overrides" },
                { text: "Processors", link: "/en/reference/processors" },
                { text: "Workflow Definitions", link: "/en/reference/workflows" },
                { text: "REST API", link: "/en/api/" },
                { text: "API Keys & MCP", link: "/en/api/api-keys-mcp" },
                {
                  text: "Platforms",
                  collapsed: true,
                  items: [
                    { text: "Overview", link: "/en/platforms/" },
                    { text: "Bilibili", link: "/en/platforms/bilibili" },
                    { text: "Douyin", link: "/en/platforms/douyin" },
                    { text: "Douyu", link: "/en/platforms/douyu" },
                    { text: "Huya", link: "/en/platforms/huya" },
                    { text: "Twitch", link: "/en/platforms/twitch" },
                    { text: "SOOP", link: "/en/platforms/soop" },
                    { text: "Bigo Live", link: "/en/platforms/bigo" },
                    { text: "TikTok", link: "/en/platforms/tiktok" },
                    { text: "Other Platforms", link: "/en/platforms/others" },
                  ],
                },
              ],
              link: "/en/reference/",
            },
            {
              text: "Operations",
              items: [
                { text: "Production Deployment", link: "/en/operations/production" },
                { text: "Storage & Capacity", link: "/en/operations/storage" },
                { text: "Backup & Restore", link: "/en/operations/backup-restore" },
                { text: "Monitoring", link: "/en/operations/monitoring" },
                { text: "Upgrading & Rollback", link: "/en/operations/upgrading" },
                { text: "Security", link: "/en/operations/security" },
                { text: "Data Governance", link: "/en/operations/data-governance" },
                { text: "Support & Versions", link: "/en/operations/support" },
              ],
            },
            {
              text: "Development",
              items: [
                { text: "Runtime Architecture", link: "/en/development/architecture" },
                { text: "Configuration Resolution", link: "/en/development/configuration" },
                { text: "Pipeline Contracts", link: "/en/development/pipeline" },
                { text: "Recording Engines", link: "/en/development/engines" },
                { text: "Mesio Internals", link: "/en/concepts/mesio" },
                { text: "Notification Internals", link: "/en/development/notifications" },
                { text: "Persistence Contracts", link: "/en/development/persistence" },
                { text: "Monitoring Implementation", link: "/en/development/monitoring" },
              ],
              link: "/en/development/",
              collapsed: true,
            },
            {
              text: "Release Notes",
              items: [
                { text: "Overview", link: "/en/release-notes/" },
                { text: "Unreleased", link: "/en/release-notes/unreleased" },
                { text: "v0.5.1", link: "/en/release-notes/v0.5.1" },
                { text: "v0.5.0", link: "/en/release-notes/v0.5.0" },
                {
                  text: "Older versions",
                  collapsed: true,
                  items: [
                    { text: "v0.4.0", link: "/en/release-notes/v0.4.0" },
                    { text: "v0.3.2", link: "/en/release-notes/v0.3.2" },
                    { text: "v0.3.1", link: "/en/release-notes/v0.3.1" },
                    { text: "v0.3.0", link: "/en/release-notes/v0.3.0" },
                    { text: "v0.2.1", link: "/en/release-notes/v0.2.1" },
                  ],
                },
              ],
            },
          ],
        },
      },
    },
    zh: {
      label: "简体中文",
      lang: "zh-CN",
      link: "/zh/",
      themeConfig: {
        editLink: {
          pattern:
            "https://github.com/hua0512/rust-srec/edit/main/rust-srec/docs/:path",
          text: "在 GitHub 上编辑此页",
        },
        lastUpdated: {
          text: "最后更新于",
          formatOptions: {
            dateStyle: "medium",
            timeStyle: "short",
          },
        },
        outline: { level: [2, 3], label: "本页内容" },
        docFooter: { prev: "上一页", next: "下一页" },
        darkModeSwitchLabel: "外观",
        lightModeSwitchTitle: "切换到浅色主题",
        darkModeSwitchTitle: "切换到深色主题",
        sidebarMenuLabel: "菜单",
        returnToTopLabel: "返回顶部",
        langMenuLabel: "切换语言",
        skipToContentLabel: "跳转到正文",
        footer: {
          message: "基于 MIT 许可证发布。",
          copyright: "Copyright © 2024-present hua0512",
        },
        nav: [
          { text: "快速开始", link: "/zh/getting-started/" },
          { text: "指南", link: "/zh/guides/" },
          { text: "参考", link: "/zh/reference/" },
          { text: "运维", link: "/zh/operations/production" },
          { text: "更新日志", link: "/zh/release-notes/" },
          { text: "捐赠", link: "/zh/donate" },
          {
            text: "v0.5.1",
            items: [
              { text: "文档版本说明", link: "/zh/operations/support" },
              { text: "v0.5.1 更新日志", link: "/zh/release-notes/v0.5.1" },
            ],
          },
        ],
        sidebar: {
          "/zh/": [
            {
              text: "快速开始",
              items: [
                { text: "介绍", link: "/zh/getting-started/" },
                { text: "安装", link: "/zh/getting-started/installation" },
                { text: "Docker 部署", link: "/zh/getting-started/docker" },
                { text: "首次录制", link: "/zh/getting-started/first-recording" },
                { text: "基础配置", link: "/zh/getting-started/configuration" },
                { text: "常见问题", link: "/zh/getting-started/faq" },
              ],
            },
            {
              text: "使用指南",
              items: [
                { text: "系统概览", link: "/zh/concepts/architecture" },
                { text: "模板与继承", link: "/zh/concepts/configuration" },
                { text: "录制时间安排", link: "/zh/guides/schedules" },
                { text: "录制引擎", link: "/zh/concepts/engines" },
                { text: "创建工作流", link: "/zh/concepts/pipeline" },
                { text: "通知", link: "/zh/concepts/notifications" },
                { text: "弹幕与统计", link: "/zh/guides/danmu" },
              ],
              link: "/zh/guides/",
            },
            {
              text: "参考",
              items: [
                { text: "设置", link: "/zh/reference/settings" },
                { text: "环境变量", link: "/zh/reference/environment" },
                { text: "文件名模板", link: "/zh/reference/filenames" },
                { text: "覆盖配置", link: "/zh/reference/configuration-overrides" },
                { text: "处理器", link: "/zh/reference/processors" },
                { text: "工作流定义", link: "/zh/reference/workflows" },
                { text: "REST API", link: "/zh/api/" },
                { text: "API 密钥与 MCP", link: "/zh/api/api-keys-mcp" },
                {
                  text: "平台支持",
                  collapsed: true,
                  items: [
                    { text: "概述", link: "/zh/platforms/" },
                    { text: "Bilibili", link: "/zh/platforms/bilibili" },
                    { text: "抖音", link: "/zh/platforms/douyin" },
                    { text: "斗鱼", link: "/zh/platforms/douyu" },
                    { text: "虎牙", link: "/zh/platforms/huya" },
                    { text: "Twitch", link: "/zh/platforms/twitch" },
                    { text: "SOOP", link: "/zh/platforms/soop" },
                    { text: "Bigo Live", link: "/zh/platforms/bigo" },
                    { text: "TikTok", link: "/zh/platforms/tiktok" },
                    { text: "其他平台", link: "/zh/platforms/others" },
                  ],
                },
              ],
              link: "/zh/reference/",
            },
            {
              text: "运维",
              items: [
                { text: "生产部署", link: "/zh/operations/production" },
                { text: "存储与容量", link: "/zh/operations/storage" },
                { text: "备份与恢复", link: "/zh/operations/backup-restore" },
                { text: "监控", link: "/zh/operations/monitoring" },
                { text: "升级与回滚", link: "/zh/operations/upgrading" },
                { text: "安全", link: "/zh/operations/security" },
                { text: "数据治理", link: "/zh/operations/data-governance" },
                { text: "支持与版本", link: "/zh/operations/support" },
              ],
            },
            {
              text: "开发",
              items: [
                { text: "运行时架构", link: "/zh/development/architecture" },
                { text: "配置解析", link: "/zh/development/configuration" },
                { text: "管道执行约定", link: "/zh/development/pipeline" },
                { text: "录制引擎内部实现", link: "/zh/development/engines" },
                { text: "Mesio 内部实现", link: "/zh/concepts/mesio" },
                { text: "通知内部实现", link: "/zh/development/notifications" },
                { text: "持久化约定", link: "/zh/development/persistence" },
                { text: "监控内部实现", link: "/zh/development/monitoring" },
              ],
              link: "/zh/development/",
              collapsed: true,
            },
            {
              text: "更新日志",
              items: [
                { text: "概览", link: "/zh/release-notes/" },
                { text: "未发布", link: "/zh/release-notes/unreleased" },
                { text: "v0.5.1", link: "/zh/release-notes/v0.5.1" },
                { text: "v0.5.0", link: "/zh/release-notes/v0.5.0" },
                {
                  text: "历史版本",
                  collapsed: true,
                  items: [
                    { text: "v0.4.0", link: "/zh/release-notes/v0.4.0" },
                    { text: "v0.3.2", link: "/zh/release-notes/v0.3.2" },
                    { text: "v0.3.1", link: "/zh/release-notes/v0.3.1" },
                    { text: "v0.3.0", link: "/zh/release-notes/v0.3.0" },
                    { text: "v0.2.1", link: "/zh/release-notes/v0.2.1" },
                  ],
                },
              ],
            },
          ],
        },
      },
    },
  },

  themeConfig: {
    logo: "/stream-rec-orange.svg",
    socialLinks: [
      { icon: "github", link: "https://github.com/hua0512/rust-srec" },
    ],
    search: {
      provider: "local",
      options: {
        locales: {
          zh: {
            translations: {
              button: {
                buttonText: "搜索",
                buttonAriaLabel: "搜索文档",
              },
              modal: {
                displayDetails: "显示详细列表",
                resetButtonTitle: "清除查询条件",
                backButtonTitle: "关闭搜索",
                noResultsText: "没有找到相关结果",
                footer: {
                  selectText: "选择",
                  selectKeyAriaLabel: "回车键",
                  navigateText: "切换",
                  navigateUpKeyAriaLabel: "向上箭头",
                  navigateDownKeyAriaLabel: "向下箭头",
                  closeText: "关闭",
                  closeKeyAriaLabel: "Esc 键",
                },
              },
            },
          },
        },
      },
    },
  },
});
