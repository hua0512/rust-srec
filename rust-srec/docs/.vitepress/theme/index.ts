import { defineComponent, h, onMounted, watch } from 'vue'
import type { Theme } from 'vitepress'
import DefaultTheme from 'vitepress/theme'
import { useData } from 'vitepress'
import { createMermaidRenderer } from 'vitepress-mermaid-renderer'
import './custom.css'

const mermaidToolbarLocales = {
    zh: {
        tooltips: {
            zoomIn: '放大',
            zoomOut: '缩小',
            resetView: '重置视图',
            copyCode: '复制代码',
            copyCodeCopied: '已复制',
            download: '下载图表',
            toggleFullscreen: '切换全屏',
            toggleToolbarExpand: '展开工具栏',
            toggleToolbarCollapse: '收起工具栏',
            renderErrorText: '图表渲染失败',
            toggleErrorDetailsText: '显示详情',
            toggleErrorDetailsHideText: '隐藏详情',
        },
    },
}

// The renderer is a client-only singleton: it stays idle during SSR and
// re-renders mounted diagrams when the colour mode or locale changes. The
// toolbar overlays a corner of the diagram, so it starts collapsed behind a
// single expand button.
const Layout = defineComponent({
    setup() {
        const { isDark, localeIndex } = useData()

        const applyMermaidSettings = () => {
            const renderer = createMermaidRenderer({
                theme: isDark.value ? 'dark' : 'default',
                startOnLoad: false,
                securityLevel: 'strict',
            })
            renderer.setToolbar({
                showLanguageLabel: false,
                downloadFormat: 'svg',
                desktop: { toggleToolbar: 'enabled', collapsed: 'collapsed' },
                mobile: { toggleToolbar: 'enabled', collapsed: 'collapsed' },
                i18n: {
                    localeIndex: localeIndex.value,
                    locales: mermaidToolbarLocales,
                },
            })
        }

        onMounted(applyMermaidSettings)
        watch([isDark, localeIndex], applyMermaidSettings)

        return () => h(DefaultTheme.Layout)
    },
})

export default {
    extends: DefaultTheme,
    Layout,
} satisfies Theme
