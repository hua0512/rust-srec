<script setup lang="ts">
import { computed, onUnmounted, ref } from 'vue'
import { useData } from 'vitepress'

const { lang } = useData()
const isZh = computed(() => lang.value.startsWith('zh'))

interface InstallTab {
  id: 'unix' | 'windows'
  label: string
  prompt: string
  command: string
}

const tabs: InstallTab[] = [
  {
    id: 'unix',
    label: 'Linux / macOS',
    prompt: '$',
    command: 'curl -fsSL https://docs.srec.rs/install.sh | bash',
  },
  {
    id: 'windows',
    label: 'Windows',
    prompt: 'PS>',
    command: 'irm https://docs.srec.rs/install.ps1 | iex',
  },
]

const activeTabId = ref<'unix' | 'windows'>('unix')
const copied = ref(false)
let copyTimer: ReturnType<typeof setTimeout> | null = null

const currentTab = computed(
  () => tabs.find((tab) => tab.id === activeTabId.value) ?? tabs[0],
)

function clearCopyTimer() {
  if (copyTimer !== null) {
    clearTimeout(copyTimer)
    copyTimer = null
  }
}

function selectTab(id: 'unix' | 'windows') {
  activeTabId.value = id
  copied.value = false
  clearCopyTimer()
}

async function copyCommand() {
  if (typeof navigator === 'undefined' || !navigator.clipboard?.writeText) {
    return
  }
  try {
    await navigator.clipboard.writeText(currentTab.value.command)
    copied.value = true
    clearCopyTimer()
    copyTimer = setTimeout(() => {
      copied.value = false
      copyTimer = null
    }, 1800)
  } catch {
    copied.value = false
  }
}

onUnmounted(() => {
  clearCopyTimer()
})
</script>

<template>
  <div class="hero-install">
    <div class="hero-install-bar">
      <div
        class="hero-install-tabs"
        role="tablist"
        :aria-label="isZh ? '选择安装平台' : 'Choose install platform'"
      >
        <button
          v-for="tab in tabs"
          :key="tab.id"
          type="button"
          role="tab"
          class="hero-install-tab"
          :class="{ 'is-active': tab.id === activeTabId }"
          :aria-selected="tab.id === activeTabId"
          @click="selectTab(tab.id)"
        >
          {{ tab.label }}
        </button>
      </div>
      <span class="hero-install-hint">
        {{ isZh ? '一键安装' : 'Quick install' }}
      </span>
    </div>

    <div class="hero-install-body">
      <div class="hero-install-code" role="tabpanel">
        <span class="hero-install-prompt" aria-hidden="true">{{ currentTab.prompt }}</span>
        <code class="hero-install-cmd">{{ currentTab.command }}</code>
      </div>

      <button
        type="button"
        class="hero-install-copy"
        :class="{ 'is-copied': copied }"
        :aria-label="
          copied
            ? isZh
              ? '已复制安装命令'
              : 'Install command copied'
            : isZh
              ? '复制安装命令'
              : 'Copy install command'
        "
        @click="copyCommand"
      >
        <svg
          v-if="copied"
          width="14"
          height="14"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2.4"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <polyline points="20 6 9 17 4 12" />
        </svg>
        <svg
          v-else
          width="14"
          height="14"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <rect x="9" y="9" width="13" height="13" rx="2" ry="2" />
          <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" />
        </svg>
        <span>{{ copied ? (isZh ? '已复制' : 'Copied') : isZh ? '复制' : 'Copy' }}</span>
      </button>
    </div>
  </div>
</template>

<style scoped>
.hero-install {
  margin: 20px auto 0;
  max-width: 520px;
  width: 100%;
  text-align: left;
  border-radius: 12px;
  border: 1px solid var(--vp-c-divider);
  background: var(--vp-c-bg-alt);
  box-shadow:
    inset 0 1px 0 rgba(255, 255, 255, 0.55),
    0 8px 24px -16px rgba(0, 0, 0, 0.14);
  overflow: hidden;
}

:global(.dark) .hero-install {
  background: rgba(24, 24, 27, 0.85);
  box-shadow:
    inset 0 1px 0 rgba(255, 255, 255, 0.05),
    0 12px 28px -16px rgba(0, 0, 0, 0.65);
}

@media (min-width: 60rem) {
  .hero-install {
    margin-left: 0;
    margin-right: 0;
  }
}

.hero-install-bar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding: 6px 10px 6px 8px;
  border-bottom: 1px solid var(--vp-c-divider);
  background: var(--vp-c-bg-soft);
}

:global(.dark) .hero-install-bar {
  background: rgba(17, 17, 20, 0.9);
}

.hero-install-tabs {
  display: inline-flex;
  align-items: center;
  gap: 4px;
}

.hero-install-tab {
  padding: 3px 9px;
  border-radius: 7px;
  border: 1px solid transparent;
  background: transparent;
  color: var(--vp-c-text-2);
  font-size: 12px;
  font-weight: 500;
  line-height: 1.4;
  cursor: pointer;
  transition:
    color 0.18s ease,
    background-color 0.18s ease,
    border-color 0.18s ease;
}

.hero-install-tab:hover {
  color: var(--vp-c-text-1);
}

.hero-install-tab.is-active {
  color: var(--vp-c-text-1);
  background: var(--vp-c-bg-elv);
  border-color: var(--vp-c-divider);
  font-weight: 600;
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.06);
}

:global(.dark) .hero-install-tab.is-active {
  background: #27272a;
  border-color: #3f3f46;
}

.hero-install-hint {
  font-size: 11px;
  font-weight: 500;
  color: var(--vp-c-text-3);
  letter-spacing: 0.02em;
}

.hero-install-body {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding: 10px 10px 10px 14px;
}

.hero-install-code {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
  flex: 1;
  overflow-x: auto;
  scrollbar-width: none;
}

.hero-install-code::-webkit-scrollbar {
  display: none;
}

.hero-install-prompt {
  font-family: var(--vp-font-family-mono);
  font-size: 12.5px;
  font-weight: 600;
  color: var(--vp-c-brand-1);
  user-select: none;
  flex-shrink: 0;
}

.hero-install-cmd {
  font-family: var(--vp-font-family-mono);
  font-size: 12.5px;
  line-height: 1.45;
  color: var(--vp-c-text-1);
  white-space: nowrap;
  background: transparent;
  padding: 0;
}

.hero-install-copy {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  flex-shrink: 0;
  padding: 5px 10px;
  border-radius: 8px;
  border: 1px solid var(--vp-c-divider);
  background: var(--vp-c-bg-elv);
  color: var(--vp-c-text-2);
  font-size: 12px;
  font-weight: 600;
  line-height: 1.2;
  cursor: pointer;
  transition:
    border-color 0.18s ease,
    color 0.18s ease,
    background-color 0.18s ease;
}

:global(.dark) .hero-install-copy {
  background: #27272a;
  border-color: #3f3f46;
}

.hero-install-copy:hover {
  border-color: var(--vp-c-brand-1);
  color: var(--vp-c-brand-1);
}

.hero-install-copy.is-copied {
  border-color: #10b981;
  color: #10b981;
  background: rgba(16, 185, 129, 0.1);
}

.hero-install-tab:focus-visible,
.hero-install-copy:focus-visible {
  outline: 2px solid var(--vp-c-brand-1);
  outline-offset: 2px;
}

@media (prefers-reduced-motion: reduce) {
  .hero-install-tab,
  .hero-install-copy {
    transition: none;
  }
}
</style>
