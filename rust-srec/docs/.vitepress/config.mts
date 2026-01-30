import { defineConfig } from 'vitepress'

export default defineConfig({
    title: 'Rust-Srec',
    description: 'Automatic Online Streaming Recorder',

    // Ignore dead links for external URLs and runtime-generated paths
    ignoreDeadLinks: [
        /^\/api\/docs/,
        /^http:\/\/localhost/,
        '/docker-compose.example.yml',
        '/env.example',
        '/env.zh.example',
    ],

    locales: {
        en: {
            label: 'English',
            lang: 'en',
            link: '/en/',
            themeConfig: {
                nav: [
                    { text: 'Home', link: '/en/' },
                    { text: 'Getting Started', link: '/en/getting-started/' },
                    { text: 'API', link: '/en/api/' },
                    { text: 'Donate', link: '/en/donate' }
                ],
                sidebar: {
                    '/en/': [
                        {
                            text: 'Getting Started',
                            items: [
                                { text: 'Introduction', link: '/en/getting-started/' },
                                { text: 'Installation', link: '/en/getting-started/installation' },
                                { text: 'Docker', link: '/en/getting-started/docker' },
                                { text: 'Configuration', link: '/en/getting-started/configuration' },
                                { text: 'FAQ', link: '/en/getting-started/faq' }
                            ]
                        },
                        {
                            text: 'Platforms',
                            items: [
                                { text: 'Overview', link: '/en/platforms/' },
                                { text: 'Bilibili', link: '/en/platforms/bilibili' },
                                { text: 'Douyin', link: '/en/platforms/douyin' },
                                { text: 'Douyu', link: '/en/platforms/douyu' },
                                { text: 'Huya', link: '/en/platforms/huya' },
                                { text: 'Twitch', link: '/en/platforms/twitch' },
                                { text: 'Other Platforms', link: '/en/platforms/others' }
                            ]
                        },
                        {
                            text: 'API Reference',
                            items: [
                                { text: 'Overview', link: '/en/api/' },
                            ]
                        },
                        {
                            text: 'Concepts',
                            items: [
                                { text: 'Architecture', link: '/en/concepts/architecture' },
                                { text: 'Configuration Layers', link: '/en/concepts/configuration' },
                                { text: 'DAG Pipeline', link: '/en/concepts/pipeline' },
                                { text: 'Notification System', link: '/en/concepts/notifications' }
                            ]
                        }
                    ]
                }
            }
        },
        zh: {
            label: '简体中文',
            lang: 'zh-CN',
            link: '/zh/',
            themeConfig: {
                nav: [
                    { text: '首页', link: '/zh/' },
                    { text: '快速开始', link: '/zh/getting-started/' },
                    { text: 'API', link: '/zh/api/' },
                    { text: '捐赠', link: '/zh/donate' }
                ],
                sidebar: {
                    '/zh/': [
                        {
                            text: '快速开始',
                            items: [
                                { text: '介绍', link: '/zh/getting-started/' },
                                { text: '安装', link: '/zh/getting-started/installation' },
                                { text: 'Docker 部署', link: '/zh/getting-started/docker' },
                                { text: '配置', link: '/zh/getting-started/configuration' },
                                { text: '常见问题', link: '/zh/getting-started/faq' }
                            ]
                        },
                        {
                            text: '平台支持',
                            items: [
                                { text: '概述', link: '/zh/platforms/' },
                                { text: 'Bilibili', link: '/zh/platforms/bilibili' },
                                { text: '抖音', link: '/zh/platforms/douyin' },
                                { text: '斗鱼', link: '/zh/platforms/douyu' },
                                { text: '虎牙', link: '/zh/platforms/huya' },
                                { text: 'Twitch', link: '/zh/platforms/twitch' },
                                { text: '其他平台', link: '/zh/platforms/others' }
                            ]
                        },
                        {
                            text: 'API 参考',
                            items: [
                                { text: '概述', link: '/zh/api/' },
                            ]
                        },
                        {
                            text: '核心概念',
                            items: [
                                { text: '系统架构', link: '/zh/concepts/architecture' },
                                { text: '配置层级', link: '/zh/concepts/configuration' },
                                { text: 'DAG 管道', link: '/zh/concepts/pipeline' },
                                { text: '通知系统', link: '/zh/concepts/notifications' }
                            ]
                        }
                    ]
                }
            }
        }
    },

    themeConfig: {
        logo: '/stream-rec.svg',
        socialLinks: [
            { icon: 'github', link: 'https://github.com/hua0512/rust-srec' }
        ],
        search: {
            provider: 'local'
        }
    }
})
