import { defineComponent, h, nextTick, onMounted, onUnmounted, provide, watch } from 'vue'
import type { Theme } from 'vitepress'
import DefaultTheme from 'vitepress/theme'
import { useData } from 'vitepress'
import { createMermaidRenderer } from 'vitepress-mermaid-renderer'

import AnimatedFlowDemo from './components/AnimatedFlowDemo.vue'
import ConfigLayersDemo from './components/ConfigLayersDemo.vue'
import DocCardGrid from './components/DocCardGrid.vue'
import HomeBottomCta from './components/HomeBottomCta.vue'
import HomeHeroBadge from './components/HomeHeroBadge.vue'
import HomeHeroInstall from './components/HomeHeroInstall.vue'
import HomeHeroVisual from './components/HomeHeroVisual.vue'
import HomeShowcase from './components/HomeShowcase.vue'
import PlatformMarquee from './components/PlatformMarquee.vue'
import StreamRepairDemo from './components/StreamRepairDemo.vue'
import SupportCards from './components/SupportCards.vue'
import TierComparison from './components/TierComparison.vue'
import WorkflowDagDemo from './components/WorkflowDagDemo.vue'

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

const lightMermaidVars = {
  fontFamily: 'Inter, system-ui, sans-serif',
  background: '#f4f4f5',
  primaryColor: '#ffffff',
  primaryTextColor: '#18181b',
  primaryBorderColor: '#d4d4d8',
  secondaryColor: '#fff4ef',
  secondaryTextColor: '#18181b',
  secondaryBorderColor: '#e44d26',
  tertiaryColor: '#fafafa',
  tertiaryTextColor: '#3f3f46',
  tertiaryBorderColor: '#e4e4e7',
  lineColor: '#71717a',
  textColor: '#18181b',
  mainBkg: '#ffffff',
  nodeBorder: '#c23b16',
  clusterBkg: '#fafafa',
  clusterBorder: '#d4d4d8',
  edgeLabelBackground: '#f4f4f5',
  actorBkg: '#ffffff',
  actorBorder: '#c23b16',
  actorTextColor: '#18181b',
  signalColor: '#52525b',
  signalTextColor: '#18181b',
  noteBkgColor: '#fff4ef',
  noteBorderColor: '#e44d26',
  noteTextColor: '#18181b',
}

const darkMermaidVars = {
  fontFamily: 'Inter, system-ui, sans-serif',
  background: '#18181b',
  primaryColor: '#27272a',
  primaryTextColor: '#fafafa',
  primaryBorderColor: '#52525b',
  secondaryColor: '#2c1812',
  secondaryTextColor: '#fafafa',
  secondaryBorderColor: '#f16529',
  tertiaryColor: '#18181b',
  tertiaryTextColor: '#e4e4e7',
  tertiaryBorderColor: '#3f3f46',
  lineColor: '#a1a1aa',
  textColor: '#fafafa',
  mainBkg: '#27272a',
  nodeBorder: '#ff7a4d',
  clusterBkg: '#111114',
  clusterBorder: '#3f3f46',
  edgeLabelBackground: '#18181b',
  actorBkg: '#27272a',
  actorBorder: '#ff7a4d',
  actorTextColor: '#fafafa',
  signalColor: '#a1a1aa',
  signalTextColor: '#fafafa',
  noteBkgColor: '#2c1812',
  noteBorderColor: '#f16529',
  noteTextColor: '#fafafa',
}

// The renderer gives every diagram box the same height and scales the diagram
// down to fit, which makes tall diagrams unreadable. Size each box to its
// diagram's aspect ratio at the column width instead, no taller than the
// diagram's natural height or 75% of the viewport; the box's CSS min-height
// still applies. Fullscreen views ignore the variable (see custom.css).
const sizeDiagramBoxes = () => {
  const boxes = document.querySelectorAll<HTMLElement>('.mermaid-container')
  if (boxes.length === 0) return

  for (const box of boxes) {
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
// re-renders mounted diagrams when theme settings change.
const Layout = defineComponent({
  setup() {
    const { isDark, localeIndex } = useData()

    const enableTransitions = () =>
      typeof document !== 'undefined' &&
      'startViewTransition' in document &&
      window.matchMedia('(prefers-reduced-motion: reduce)').matches === false

    provide('toggle-appearance', async (event?: MouseEvent) => {
      if (!enableTransitions()) {
        isDark.value = !isDark.value
        return
      }
      const x = event?.clientX ?? window.innerWidth / 2
      const y = event?.clientY ?? window.innerHeight / 2
      const endRadius = Math.hypot(
        Math.max(x, window.innerWidth - x),
        Math.max(y, window.innerHeight - y),
      )
      const root = document.documentElement
      root.style.setProperty('--reveal-x', `${x}px`)
      root.style.setProperty('--reveal-y', `${y}px`)
      root.style.setProperty('--reveal-r', `${endRadius}px`)
      const transition = (
        document as Document & {
          startViewTransition: (cb: () => Promise<void>) => { ready: Promise<void> }
        }
      ).startViewTransition(async () => {
        isDark.value = !isDark.value
        await nextTick()
      })
      await transition.ready
    })

    const applyMermaidTheme = () => {
      createMermaidRenderer({
        theme: 'base',
        themeVariables: isDark.value ? darkMermaidVars : lightMermaidVars,
        startOnLoad: false,
        securityLevel: 'strict',
      })
    }

    const applyMermaidToolbar = () => {
      createMermaidRenderer().setToolbar({
        showLanguageLabel: false,
        downloadFormat: 'svg',
        fullscreenMode: 'dialog',
        desktop: { toggleToolbar: 'enabled', collapsed: 'collapsed' },
        mobile: { toggleToolbar: 'enabled', collapsed: 'collapsed' },
        i18n: {
          localeIndex: localeIndex.value,
          locales: mermaidToolbarLocales,
        },
      })
    }

    onMounted(() => {
      applyMermaidTheme()
      applyMermaidToolbar()
    })
    watch(isDark, applyMermaidTheme)
    watch(localeIndex, applyMermaidToolbar)

    // Diagrams render lazily and re-render on theme changes, so re-size
    // whenever the content DOM or viewport changes.
    let frame = 0
    const scheduleSizing = () => {
      if (!document.querySelector('.mermaid-container')) return
      cancelAnimationFrame(frame)
      frame = requestAnimationFrame(sizeDiagramBoxes)
    }

    let observer: MutationObserver | undefined
    onMounted(() => {
      const target = document.getElementById('VPContent') ?? document.body
      observer = new MutationObserver(scheduleSizing)
      observer.observe(target, { childList: true, subtree: true })
      window.addEventListener('resize', scheduleSizing)
    })
    onUnmounted(() => {
      observer?.disconnect()
      window.removeEventListener('resize', scheduleSizing)
      cancelAnimationFrame(frame)
    })

    return () =>
      h(DefaultTheme.Layout, null, {
        'home-hero-info-before': () => h(HomeHeroBadge),
        'home-hero-actions-after': () => h(HomeHeroInstall),
        'home-hero-image': () => h(HomeHeroVisual),
      })
  },
})

export default {
  extends: DefaultTheme,
  Layout,
  enhanceApp({ app }) {
    app.component('AnimatedFlowDemo', AnimatedFlowDemo)
    app.component('HomeShowcase', HomeShowcase)
    app.component('PlatformMarquee', PlatformMarquee)
    app.component('TierComparison', TierComparison)
    app.component('ConfigLayersDemo', ConfigLayersDemo)
    app.component('WorkflowDagDemo', WorkflowDagDemo)
    app.component('StreamRepairDemo', StreamRepairDemo)
    app.component('DocCardGrid', DocCardGrid)
    app.component('SupportCards', SupportCards)
    app.component('HomeBottomCta', HomeBottomCta)
  },
} satisfies Theme
