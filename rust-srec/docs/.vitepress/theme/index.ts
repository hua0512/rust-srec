import { defineComponent, h, onMounted, onUnmounted, watch } from 'vue'
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

// The renderer gives every diagram box the same height and scales the diagram
// down to fit, which makes tall diagrams unreadable. Size each box to its
// diagram's aspect ratio at the column width instead, no taller than the
// diagram's natural height or 75% of the viewport; the box's CSS min-height
// still applies. The renderer re-fits a diagram whenever its box resizes.
// Fullscreen views ignore the variable (see custom.css).
const sizeDiagramBoxes = () => {
    for (const box of document.querySelectorAll<HTMLElement>('.mermaid-container')) {
        // Toolbar icons are SVGs too; the diagram is the one Mermaid labels.
        const diagram = box.querySelector<SVGSVGElement>('svg[aria-roledescription]')
        const { width, height } = diagram?.viewBox.baseVal ?? { width: 0, height: 0 }
        if (width === 0 || height === 0) continue
        const fitted = Math.min(
            (box.clientWidth * height) / width,
            height,
            window.innerHeight * 0.75,
        )
        box.style.setProperty('--diagram-height', `${Math.round(fitted)}px`)
    }
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

        // Diagrams render lazily and re-render on theme changes, so re-size
        // whenever the DOM or the viewport changes.
        let frame = 0
        const scheduleSizing = () => {
            cancelAnimationFrame(frame)
            frame = requestAnimationFrame(sizeDiagramBoxes)
        }
        let observer: MutationObserver | undefined
        onMounted(() => {
            observer = new MutationObserver(scheduleSizing)
            observer.observe(document.body, { childList: true, subtree: true })
            window.addEventListener('resize', scheduleSizing)
        })
        onUnmounted(() => {
            observer?.disconnect()
            window.removeEventListener('resize', scheduleSizing)
            cancelAnimationFrame(frame)
        })

        return () => h(DefaultTheme.Layout)
    },
})

export default {
    extends: DefaultTheme,
    Layout,
} satisfies Theme
