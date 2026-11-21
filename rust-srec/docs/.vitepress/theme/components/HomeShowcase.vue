<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { useData, withBase } from 'vitepress'

const { isDark, lang } = useData()
const isZh = computed(() => lang.value.startsWith('zh'))

interface ShowcaseItem {
  slug: string
  tabEn: string
  tabZh: string
  captionEn: string
  captionZh: string
  altEn: string
  altZh: string
  urlPath: string
}

const items: ShowcaseItem[] = [
  {
    slug: 'dashboard',
    tabEn: 'Dashboard',
    tabZh: '仪表盘',
    captionEn: 'Live recordings, system health, storage overview, and pipeline throughput at a glance.',
    captionZh: '一览正在进行的录制、系统健康状态、存储占用与后处理吞吐量。',
    altEn: 'Dashboard with live recordings and pipeline statistics',
    altZh: '仪表盘：正在录制的直播与处理任务统计',
    urlPath: '/dashboard',
  },
  {
    slug: 'player',
    tabEn: 'Danmu Player',
    tabZh: '弹幕播放器',
    captionEn: 'Watch recorded streams with synchronized Danmu and live chat scrolling over the video.',
    captionZh: '内置播放器直接回放录像，弹幕与聊天记录同步叠加在画面上。',
    altEn: 'Recording playing with chat scrolling over the video',
    altZh: '回放录像，弹幕在画面上滚动',
    urlPath: '/player',
  },
  {
    slug: 'streamers',
    tabEn: 'Streamers',
    tabZh: '主播列表',
    captionEn: 'Track every followed channel with live status, real-time download speed, and recorded size.',
    captionZh: '统一管理所有关注频道，开播主播实时显示下载速度与文件大小。',
    altEn: 'Streamer list showing active and offline channels',
    altZh: '主播列表',
    urlPath: '/streamers',
  },
  {
    slug: 'session',
    tabEn: 'Session Detail',
    tabZh: '场次详情',
    captionEn: 'Inspect each recorded broadcast with playable segment previews, metadata, and file actions.',
    captionZh: '查看每场直播录制的分段预览、元数据以及快速操作。',
    altEn: 'A recorded session page with playable preview',
    altZh: '录制会话详情页',
    urlPath: '/sessions/detail',
  },
  {
    slug: 'danmu-stats',
    tabEn: 'Danmu Stats',
    tabZh: '弹幕统计',
    captionEn: 'Review chat activity timelines, highlight peaks, top chatters, and top keywords per session.',
    captionZh: '查看每场直播的弹幕走势曲线、高能时段、活跃观众与高频热词。',
    altEn: 'Chat statistics for a recorded session',
    altZh: '场次弹幕统计',
    urlPath: '/sessions/danmu-stats',
  },
  {
    slug: 'workflow',
    tabEn: 'Workflow Graph',
    tabZh: '工作流编排',
    captionEn: 'Build multi-step post-processing DAGs visually: remux, thumbnails, cloud upload, and cleanup.',
    captionZh: '可视化拖拽编排多步骤后处理工作流：封装、封面、云端上传与清理。',
    altEn: 'Workflow editor in graph view',
    altZh: '工作流编辑器（图视图）',
    urlPath: '/workflows/editor',
  },
  {
    slug: 'jobs',
    tabEn: 'Pipeline Jobs',
    tabZh: '任务队列',
    captionEn: 'Follow every running, queued, completed, or retried processing job with live logs.',
    captionZh: '实时追踪每个运行中、排队中、已完成或重试的后处理任务与日志。',
    altEn: 'Pipeline jobs list',
    altZh: '处理任务列表',
    urlPath: '/pipeline/jobs',
  },
  {
    slug: 'mobile',
    tabEn: 'Mobile View',
    tabZh: '移动端',
    captionEn: 'Responsive web interface lets you check live recordings and jobs from your phone.',
    captionZh: '自适应移动端界面，在手机浏览器里随时查看录制与任务状态。',
    altEn: 'The web interface on a mobile phone',
    altZh: '手机上的 Web 界面',
    urlPath: '/mobile',
  },
]

const activeIndex = ref(0)
const lightboxOpen = ref(false)
const previewTheme = ref<'auto' | 'light' | 'dark'>('auto')

const currentItem = computed(() => items[activeIndex.value])

const effectiveTheme = computed<'light' | 'dark'>(() => {
  if (previewTheme.value === 'auto') {
    return isDark.value ? 'dark' : 'light'
  }
  return previewTheme.value
})

const currentSrc = computed(() => {
  const localeDir = isZh.value ? 'zh-CN' : 'en'
  return withBase(`/images/showcase/${localeDir}/${currentItem.value.slug}-${effectiveTheme.value}.webp`)
})

function togglePreviewTheme() {
  previewTheme.value = effectiveTheme.value === 'dark' ? 'light' : 'dark'
}

function selectTab(index: number) {
  activeIndex.value = index
}

function prev() {
  activeIndex.value = (activeIndex.value - 1 + items.length) % items.length
}

function next() {
  activeIndex.value = (activeIndex.value + 1) % items.length
}

function onTabKeydown(event: KeyboardEvent) {
  if (event.key === 'ArrowRight') {
    event.preventDefault()
    next()
  } else if (event.key === 'ArrowLeft') {
    event.preventDefault()
    prev()
  }
}

function openLightbox() {
  lightboxOpen.value = true
}

function closeLightbox() {
  lightboxOpen.value = false
}

function onWindowKeydown(event: KeyboardEvent) {
  if (!lightboxOpen.value) return
  if (event.key === 'Escape') {
    closeLightbox()
  } else if (event.key === 'ArrowRight') {
    next()
  } else if (event.key === 'ArrowLeft') {
    prev()
  }
}

onMounted(() => {
  window.addEventListener('keydown', onWindowKeydown)
})

onUnmounted(() => {
  window.removeEventListener('keydown', onWindowKeydown)
})
</script>

<template>
  <div class="showcase-root">
    <div
      class="showcase-tabs"
      role="tablist"
      :aria-label="isZh ? '界面预览切换' : 'Interface screenshots'"
      @keydown="onTabKeydown"
    >
      <button
        v-for="(item, idx) in items"
        :key="item.slug"
        type="button"
        role="tab"
        class="showcase-tab"
        :class="{ 'is-active': idx === activeIndex }"
        :aria-selected="idx === activeIndex"
        @click="selectTab(idx)"
      >
        {{ isZh ? item.tabZh : item.tabEn }}
      </button>
    </div>

    <div class="showcase-window">
      <div class="showcase-chrome">
        <div class="showcase-dots" aria-hidden="true">
          <span class="dot dot-red" />
          <span class="dot dot-amber" />
          <span class="dot dot-green" />
        </div>
        <div class="showcase-url">
          <span>rust-srec.local{{ currentItem.urlPath }}</span>
        </div>
        <div class="showcase-chrome-actions">
          <button
            type="button"
            class="chrome-btn chrome-theme-btn"
            :aria-label="
              effectiveTheme === 'dark'
                ? isZh
                  ? '切换为浅色界面截图预览'
                  : 'Preview light UI screenshot'
                : isZh
                  ? '切换为深色界面截图预览'
                  : 'Preview dark UI screenshot'
            "
            :title="
              effectiveTheme === 'dark'
                ? isZh
                  ? '切换浅色截图'
                  : 'Switch to light preview'
                : isZh
                  ? '切换深色截图'
                  : 'Switch to dark preview'
            "
            @click="togglePreviewTheme"
          >
            <svg
              v-if="effectiveTheme === 'dark'"
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
              <circle cx="12" cy="12" r="4" />
              <path d="M12 2v2" />
              <path d="M12 20v2" />
              <path d="m4.93 4.93 1.41 1.41" />
              <path d="m17.66 17.66 1.41 1.41" />
              <path d="M2 12h2" />
              <path d="M20 12h2" />
              <path d="m6.34 17.66-1.41 1.41" />
              <path d="m19.07 4.93-1.41 1.41" />
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
              <path d="M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9Z" />
            </svg>
            <span class="chrome-theme-label">
              {{ effectiveTheme === 'dark' ? (isZh ? '深色' : 'Dark') : isZh ? '浅色' : 'Light' }}
            </span>
          </button>

          <span class="chrome-divider" aria-hidden="true" />

          <button
            type="button"
            class="chrome-btn"
            :aria-label="isZh ? '上一张' : 'Previous screenshot'"
            @click="prev"
          >
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
              <polyline points="15 18 9 12 15 6" />
            </svg>
          </button>
          <span class="showcase-counter">{{ activeIndex + 1 }} / {{ items.length }}</span>
          <button
            type="button"
            class="chrome-btn"
            :aria-label="isZh ? '下一张' : 'Next screenshot'"
            @click="next"
          >
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
              <polyline points="9 18 15 12 9 6" />
            </svg>
          </button>
          <button
            type="button"
            class="chrome-btn"
            :aria-label="isZh ? '放大查看' : 'Zoom screenshot'"
            @click="openLightbox"
          >
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
              <polyline points="15 3 21 3 21 9" />
              <polyline points="9 21 3 21 3 15" />
              <line x1="21" y1="3" x2="14" y2="10" />
              <line x1="3" y1="21" x2="10" y2="14" />
            </svg>
          </button>
        </div>
      </div>

      <div class="showcase-stage" role="tabpanel">
        <button
          type="button"
          class="showcase-image-trigger"
          :aria-label="isZh ? '点击放大预览图片' : 'Click to expand screenshot'"
          @click="openLightbox"
        >
          <Transition name="showcase-fade" mode="out-in">
            <img
              :key="currentSrc"
              :src="currentSrc"
              :alt="isZh ? currentItem.altZh : currentItem.altEn"
              class="showcase-img"
              loading="lazy"
            />
          </Transition>
        </button>
      </div>

      <div class="showcase-caption-bar">
        <p class="showcase-caption">
          {{ isZh ? currentItem.captionZh : currentItem.captionEn }}
        </p>
        <div class="showcase-meta-right">
          <div
            class="showcase-pagination"
            role="group"
            :aria-label="isZh ? '截图分页' : 'Screenshot pagination'"
          >
            <button
              v-for="(item, idx) in items"
              :key="item.slug"
              type="button"
              class="pagination-dot"
              :class="{ 'is-active': idx === activeIndex }"
              :aria-label="isZh ? `查看${item.tabZh}` : `View ${item.tabEn}`"
              :aria-current="idx === activeIndex ? 'true' : undefined"
              @click="selectTab(idx)"
            />
          </div>
          <span class="showcase-hint">
            {{ isZh ? '点击放大 · ← / → 切换' : 'Click to zoom · ← / → keys' }}
          </span>
        </div>
      </div>
    </div>

    <Transition name="lightbox-fade">
      <div
        v-if="lightboxOpen"
        class="showcase-lightbox"
        role="dialog"
        aria-modal="true"
        :aria-label="isZh ? currentItem.tabZh : currentItem.tabEn"
        @click.self="closeLightbox"
      >
        <div class="lightbox-inner">
          <div class="lightbox-top">
            <div class="lightbox-title-group">
              <span class="lightbox-title">
                {{ isZh ? currentItem.tabZh : currentItem.tabEn }}
              </span>
              <span class="lightbox-badge">{{ activeIndex + 1 }} / {{ items.length }}</span>
            </div>
            <div class="lightbox-controls">
              <button
                type="button"
                class="lightbox-btn lightbox-theme-btn"
                :aria-label="
                  effectiveTheme === 'dark'
                    ? isZh
                      ? '切换为浅色截图'
                      : 'Switch to light preview'
                    : isZh
                      ? '切换为深色截图'
                      : 'Switch to dark preview'
                "
                @click="togglePreviewTheme"
              >
                <svg
                  v-if="effectiveTheme === 'dark'"
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
                  <circle cx="12" cy="12" r="4" />
                  <path d="M12 2v2" />
                  <path d="M12 20v2" />
                  <path d="m4.93 4.93 1.41 1.41" />
                  <path d="m17.66 17.66 1.41 1.41" />
                  <path d="M2 12h2" />
                  <path d="M20 12h2" />
                  <path d="m6.34 17.66-1.41 1.41" />
                  <path d="m19.07 4.93-1.41 1.41" />
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
                  <path d="M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9Z" />
                </svg>
                <span>{{ effectiveTheme === 'dark' ? (isZh ? '深色' : 'Dark') : isZh ? '浅色' : 'Light' }}</span>
              </button>
              <button
                type="button"
                class="lightbox-btn"
                :aria-label="isZh ? '上一张' : 'Previous'"
                @click="prev"
              >
                <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                  <polyline points="15 18 9 12 15 6" />
                </svg>
              </button>
              <button
                type="button"
                class="lightbox-btn"
                :aria-label="isZh ? '下一张' : 'Next'"
                @click="next"
              >
                <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                  <polyline points="9 18 15 12 9 6" />
                </svg>
              </button>
              <button
                type="button"
                class="lightbox-btn"
                :aria-label="isZh ? '关闭预览' : 'Close preview'"
                @click="closeLightbox"
              >
                <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                  <line x1="18" y1="6" x2="6" y2="18" />
                  <line x1="6" y1="6" x2="18" y2="18" />
                </svg>
              </button>
            </div>
          </div>

          <div class="lightbox-media">
            <Transition name="showcase-fade" mode="out-in">
              <img
                :key="currentSrc"
                :src="currentSrc"
                :alt="isZh ? currentItem.altZh : currentItem.altEn"
                class="lightbox-img"
              />
            </Transition>
          </div>

          <div class="lightbox-footer">
            <p class="lightbox-caption">
              {{ isZh ? currentItem.captionZh : currentItem.captionEn }}
            </p>
            <div class="lightbox-dots" role="group" :aria-label="isZh ? '截图分页' : 'Screenshot pagination'">
              <button
                v-for="(item, idx) in items"
                :key="item.slug"
                type="button"
                class="lightbox-dot"
                :class="{ 'is-active': idx === activeIndex }"
                :aria-label="isZh ? `查看${item.tabZh}` : `View ${item.tabEn}`"
                @click="selectTab(idx)"
              />
            </div>
          </div>
        </div>
      </div>
    </Transition>
  </div>
</template>

<style scoped>
.showcase-root {
  display: flex;
  flex-direction: column;
  gap: 16px;
}

.showcase-tabs {
  display: flex;
  flex-wrap: wrap;
  justify-content: center;
  gap: 8px;
}

.showcase-tab {
  padding: 7px 14px;
  border-radius: 999px;
  border: 1px solid var(--vp-c-divider);
  background: var(--vp-c-bg-soft);
  color: var(--vp-c-text-2);
  font-size: 13px;
  font-weight: 500;
  cursor: pointer;
  transition:
    border-color 0.2s ease,
    background-color 0.2s ease,
    color 0.2s ease;
}

.showcase-tab:hover {
  border-color: var(--vp-c-brand-1);
  color: var(--vp-c-text-1);
}

.showcase-tab.is-active {
  border-color: var(--vp-c-brand-1);
  background: var(--vp-c-brand-soft);
  color: var(--vp-c-brand-1);
  font-weight: 600;
}

.showcase-window {
  border: 1px solid var(--vp-c-divider);
  border-radius: 16px;
  overflow: hidden;
  background: var(--vp-c-bg-soft);
  box-shadow:
    0 24px 56px -24px rgba(0, 0, 0, 0.24),
    -20px 10px 48px -28px rgba(228, 77, 38, 0.26),
    20px 10px 48px -28px rgba(124, 58, 237, 0.24);
}

:global(.dark) .showcase-window {
  border-color: rgba(255, 255, 255, 0.1);
  box-shadow:
    0 26px 64px -22px rgba(0, 0, 0, 0.65),
    -24px 12px 56px -26px rgba(241, 101, 41, 0.3),
    24px 12px 56px -26px rgba(139, 92, 246, 0.28);
}

.showcase-chrome {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 10px 16px;
  border-bottom: 1px solid var(--vp-c-divider);
  background: var(--vp-c-bg-alt);
}

.showcase-dots {
  display: flex;
  align-items: center;
  gap: 6px;
}

.dot {
  width: 10px;
  height: 10px;
  border-radius: 50%;
}

.dot-red {
  background: #ef4444;
  opacity: 0.78;
}

.dot-amber {
  background: #f59e0b;
  opacity: 0.78;
}

.dot-green {
  background: #22c55e;
  opacity: 0.78;
}

.showcase-url {
  flex: 1;
  max-width: 340px;
  text-align: center;
  padding: 4px 12px;
  border-radius: 8px;
  border: 1px solid var(--vp-c-divider);
  background: var(--vp-c-bg);
  color: var(--vp-c-text-2);
  font-family: var(--vp-font-family-mono);
  font-size: 12px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.showcase-chrome-actions {
  display: flex;
  align-items: center;
  gap: 6px;
}

.chrome-divider {
  width: 1px;
  height: 16px;
  background: var(--vp-c-divider);
  margin: 0 2px;
}

.showcase-counter {
  font-family: var(--vp-font-family-mono);
  font-size: 12px;
  color: var(--vp-c-text-2);
  min-width: 38px;
  text-align: center;
}

.chrome-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 28px;
  height: 28px;
  border-radius: 7px;
  border: 1px solid var(--vp-c-divider);
  background: var(--vp-c-bg);
  color: var(--vp-c-text-2);
  cursor: pointer;
  transition: border-color 0.2s ease, color 0.2s ease, background-color 0.2s ease;
}

.chrome-btn:hover {
  border-color: var(--vp-c-brand-1);
  color: var(--vp-c-brand-1);
}

.chrome-theme-btn {
  width: auto;
  padding: 0 9px;
  gap: 5px;
  font-size: 11.5px;
  font-weight: 600;
  color: var(--vp-c-text-1);
}

.chrome-theme-btn:hover {
  background: var(--vp-c-brand-soft);
}

.showcase-stage {
  background: var(--vp-c-bg);
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 0;
}

.showcase-image-trigger {
  width: 100%;
  display: flex;
  align-items: center;
  justify-content: center;
  border: none;
  background: transparent;
  padding: 0;
  cursor: zoom-in;
}

.showcase-img {
  display: block;
  width: 100%;
  height: auto;
  max-height: 640px;
  object-fit: contain;
}

.showcase-caption-bar {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  justify-content: space-between;
  gap: 10px 16px;
  padding: 12px 18px;
  border-top: 1px solid var(--vp-c-divider);
  background: var(--vp-c-bg-alt);
}

.showcase-caption {
  margin: 0 !important;
  font-size: 14px;
  color: var(--vp-c-text-1);
  font-weight: 500;
  flex: 1 1 320px;
}

.showcase-meta-right {
  display: flex;
  align-items: center;
  gap: 14px;
  margin-left: auto;
}

.showcase-pagination {
  display: inline-flex;
  align-items: center;
  gap: 6px;
}

.pagination-dot {
  width: 7px;
  height: 7px;
  padding: 0;
  border: none;
  border-radius: 999px;
  background: var(--vp-c-border);
  cursor: pointer;
  transition: width 0.22s ease, background-color 0.22s ease;
}

.pagination-dot:hover {
  background: var(--vp-c-text-2);
}

.pagination-dot.is-active {
  width: 20px;
  background: linear-gradient(90deg, var(--vp-c-brand-1), #8b5cf6);
}

.showcase-hint {
  font-size: 12px;
  color: var(--vp-c-text-3);
  white-space: nowrap;
}

.showcase-fade-enter-active,
.showcase-fade-leave-active {
  transition: opacity 0.18s ease;
}

.showcase-fade-enter-from,
.showcase-fade-leave-to {
  opacity: 0;
}

/* Lightbox modal */
.lightbox-fade-enter-active,
.lightbox-fade-leave-active {
  transition: opacity 0.2s ease;
}

.lightbox-fade-enter-from,
.lightbox-fade-leave-to {
  opacity: 0;
}

.showcase-lightbox {
  position: fixed;
  inset: 0;
  z-index: 200;
  background: rgba(9, 9, 11, 0.86);
  backdrop-filter: blur(12px);
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 20px;
}

.lightbox-inner {
  width: min(1240px, 100%);
  max-height: 92vh;
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding: 16px;
  border-radius: 16px;
  border: 1px solid rgba(255, 255, 255, 0.14);
  background: linear-gradient(180deg, rgba(24, 24, 27, 0.96), rgba(14, 14, 17, 0.98));
  box-shadow:
    0 32px 80px -20px rgba(0, 0, 0, 0.78),
    -18px 0 60px -30px rgba(241, 101, 41, 0.28),
    18px 0 60px -30px rgba(139, 92, 246, 0.26);
  color: #fafafa;
}

.lightbox-top {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
}

.lightbox-title-group {
  display: flex;
  align-items: center;
  gap: 8px;
}

.lightbox-title {
  font-size: 14px;
  font-weight: 600;
  color: #fafafa;
}

.lightbox-badge {
  padding: 2px 8px;
  border-radius: 999px;
  background: rgba(255, 255, 255, 0.08);
  border: 1px solid rgba(255, 255, 255, 0.12);
  font-family: var(--vp-font-family-mono);
  font-size: 11px;
  color: #d4d4d8;
}

.lightbox-controls {
  display: flex;
  align-items: center;
  gap: 8px;
}

.lightbox-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 34px;
  height: 34px;
  border-radius: 8px;
  border: 1px solid rgba(255, 255, 255, 0.16);
  background: rgba(255, 255, 255, 0.06);
  color: #fafafa;
  font-size: 13px;
  cursor: pointer;
  transition: background-color 0.15s ease, border-color 0.15s ease;
}

.lightbox-theme-btn {
  width: auto;
  padding: 0 10px;
  gap: 6px;
  font-size: 12px;
  font-weight: 600;
}

.lightbox-btn:hover {
  background: rgba(241, 101, 41, 0.24);
  border-color: #ff7a4d;
}

.lightbox-media {
  display: flex;
  align-items: center;
  justify-content: center;
  overflow: hidden;
  min-height: 0;
  border-radius: 10px;
  border: 1px solid rgba(255, 255, 255, 0.08);
  background: #09090b;
}

.lightbox-img {
  display: block;
  max-width: 100%;
  max-height: 72vh;
  object-fit: contain;
}

.lightbox-footer {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 10px;
}

.lightbox-caption {
  margin: 0 !important;
  text-align: center;
  font-size: 13px;
  color: #a1a1aa;
}

.lightbox-dots {
  display: inline-flex;
  align-items: center;
  gap: 6px;
}

.lightbox-dot {
  width: 7px;
  height: 7px;
  padding: 0;
  border: none;
  border-radius: 999px;
  background: rgba(255, 255, 255, 0.26);
  cursor: pointer;
  transition: width 0.2s ease, background-color 0.2s ease;
}

.lightbox-dot.is-active {
  width: 20px;
  background: linear-gradient(90deg, #ff7a4d, #a78bfa);
}

@media (max-width: 720px) {
  .showcase-url {
    display: none;
  }

  .showcase-hint {
    display: none;
  }

  .showcase-meta-right {
    margin-left: 0;
  }
}

@media (max-width: 480px) {
  .chrome-theme-label {
    display: none;
  }

  .chrome-theme-btn {
    width: 28px;
    padding: 0;
  }
}

@media (prefers-reduced-motion: reduce) {
  .showcase-fade-enter-active,
  .showcase-fade-leave-active,
  .lightbox-fade-enter-active,
  .lightbox-fade-leave-active,
  .pagination-dot,
  .lightbox-dot {
    transition: none;
  }
}
</style>
