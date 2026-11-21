<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { useData } from 'vitepress'

const props = withDefaults(
  defineProps<{
    compact?: boolean
  }>(),
  {
    compact: false,
  },
)

const { lang } = useData()
const isZh = computed(() => lang.value.startsWith('zh'))

interface StageInfo {
  id: 'detect' | 'record' | 'postprocess' | 'notify'
  step: string
  titleEn: string
  titleZh: string
  shortEn: string
  shortZh: string
  descEn: string
  descZh: string
  highlightsEn: string[]
  highlightsZh: string[]
  statusEn: string
  statusZh: string
}

const stages: StageInfo[] = [
  {
    id: 'detect',
    step: '01',
    titleEn: 'Detect Live',
    titleZh: '开播监测',
    shortEn: 'Poll & schedule',
    shortZh: '轮询与时间窗',
    descEn:
      'Monitors every followed channel on your schedule, uses batch APIs where available, and respects filters like title keywords or active time windows.',
    descZh:
      '按设定的频率与时间窗口自动监测关注的主播，在支持的平台上自动合并批量查询，并按标题关键词等规则判断是否开录。',
    highlightsEn: ['14+ platforms', 'Time & keyword filters', 'Batch live checks'],
    highlightsZh: ['14+ 平台支持', '时间段与关键词过滤', '批量开播检测'],
    statusEn: 'Channel went live · 1080p60 stream ready',
    statusZh: '检测到开播 · 1080p60 源流就绪',
  },
  {
    id: 'record',
    step: '02',
    titleEn: 'Record & Repair',
    titleZh: '录制与实时修复',
    shortEn: 'Stream + Danmu',
    shortZh: '视频流与弹幕同步',
    descEn:
      'Captures the live FLV/HLS stream with Mesio, FFmpeg, or Streamlink while saving synced Danmu chat and repairing broken timestamps or stalls on the fly.',
    descZh:
      '使用 Mesio、FFmpeg 或 Streamlink 抓取直播流，同步保存实时弹幕，并在录制过程中自动修复时间戳跳变与流卡顿。',
    highlightsEn: ['Mesio live repair', 'Synced Danmu XML', 'Clean playable files'],
    highlightsZh: ['Mesio 实时修复', '同步弹幕保存', '成品即开即播'],
    statusEn: 'Recording · 6.4 MB/s · 0 timestamp errors',
    statusZh: '正在录制 · 6.4 MB/s · 时间戳正常',
  },
  {
    id: 'postprocess',
    step: '03',
    titleEn: 'Post-Process DAG',
    titleZh: '工作流后处理',
    shortEn: 'Automate cleanup',
    shortZh: '自动转码与上传',
    descEn:
      'Runs your visual workflow graph automatically when a segment or session finishes: remux to MP4, generate thumbnails, upload via rclone or Baidu Netdisk, and clean up safely.',
    descZh:
      '分段或场次结束后自动触发可视化工作流：封装转码、生成预览图、上传至 rclone 或百度网盘，仅在上游成功后清理本地文件。',
    highlightsEn: ['Remux & thumbnails', 'rclone & Baidu Netdisk', 'Failure-safe cleanup'],
    highlightsZh: ['封装与封面生成', 'rclone / 百度网盘', '失败保护不误删'],
    statusEn: 'Remuxed -> Uploaded -> Local cache cleaned',
    statusZh: '封装完成 -> 上传成功 -> 本地已清理',
  },
  {
    id: 'notify',
    step: '04',
    titleEn: 'Notify & Replay',
    titleZh: '通知与回放',
    shortEn: 'Alerts & player',
    shortZh: '推送与弹幕回放',
    descEn:
      'Sends alerts via Telegram, Discord, Gotify, Email, or Webhooks, and lets you browse sessions, review chat statistics, and replay recordings with scrolling Danmu in the web player.',
    descZh:
      '通过 Telegram、Discord、Gotify、邮件或 Webhook 及时推送状态，并在网页端查看弹幕统计与热词、带滚动弹幕直接回放录像。',
    highlightsEn: ['Multi-channel alerts', 'Built-in Danmu player', 'Session analytics'],
    highlightsZh: ['多渠道消息推送', '内置弹幕播放器', '场次弹幕统计'],
    statusEn: 'Session archived · 14,820 chat messages indexed',
    statusZh: '场次已归档 · 已收录 14,820 条弹幕',
  },
]

const pollRows = [
  { platform: 'Bilibili', status: 'LIVE', live: true, accent: '#fb7299' },
  { platform: 'Douyin', status: 'LIVE', live: true, accent: '#fe2c55' },
  { platform: 'Twitch', status: 'Offline', live: false, accent: '#9146ff' },
]

const equalizerHeights = [42, 78, 56, 94, 64, 86, 48, 72, 60]

const activeIndex = ref(0)
const isPlaying = ref(true)
const progressSeed = ref(0)
let timer: ReturnType<typeof setInterval> | null = null

const activeStage = computed(() => stages[activeIndex.value])

function stopTimer() {
  if (timer !== null) {
    clearInterval(timer)
    timer = null
  }
}

function startTimer() {
  stopTimer()
  progressSeed.value += 1
  timer = setInterval(() => {
    activeIndex.value = (activeIndex.value + 1) % stages.length
  }, 4000)
}

function selectStage(index: number) {
  activeIndex.value = index
  if (isPlaying.value) {
    startTimer()
  }
}

function togglePlay() {
  isPlaying.value = !isPlaying.value
  if (isPlaying.value) {
    startTimer()
  } else {
    stopTimer()
  }
}

onMounted(() => {
  const prefersReduced =
    typeof window !== 'undefined' &&
    window.matchMedia('(prefers-reduced-motion: reduce)').matches
  if (prefersReduced) {
    isPlaying.value = false
  } else {
    startTimer()
  }
})

onUnmounted(() => {
  stopTimer()
})
</script>

<template>
  <div class="flow-demo" :class="{ 'is-compact': props.compact }">
    <div class="flow-header">
      <div class="flow-heading">
        <span class="flow-eyebrow">
          {{ isZh ? '全自动录制闭环' : 'END-TO-END LIFECYCLE' }}
        </span>
        <span class="flow-caption">
          {{
            isZh
              ? '点击任意阶段查看细节，或自动轮播了解 Rust-Srec 如何接管整场直播'
              : 'Select any stage to inspect how Rust-Srec handles a broadcast from start to archive'
          }}
        </span>
      </div>
      <button
        type="button"
        class="flow-toggle"
        :aria-pressed="isPlaying"
        @click="togglePlay"
      >
        <svg v-if="isPlaying" width="14" height="14" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
          <rect x="6" y="5" width="4" height="14" rx="1" />
          <rect x="14" y="5" width="4" height="14" rx="1" />
        </svg>
        <svg v-else width="14" height="14" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
          <path d="M8 5.14v14l11-7-11-7z" />
        </svg>
        <span>{{ isPlaying ? (isZh ? '暂停' : 'Pause') : isZh ? '播放' : 'Play' }}</span>
      </button>
    </div>

    <div class="flow-pipeline" role="tablist" :aria-label="isZh ? '录制生命周期阶段' : 'Recording lifecycle stages'">
      <template v-for="(stage, idx) in stages" :key="stage.id">
        <button
          type="button"
          role="tab"
          class="flow-node"
          :class="{
            'is-active': idx === activeIndex,
            'is-complete': idx < activeIndex,
          }"
          :aria-selected="idx === activeIndex"
          @click="selectStage(idx)"
        >
          <span class="flow-node-top">
            <span class="flow-step">{{ stage.step }}</span>
            <span class="flow-icon" aria-hidden="true">
              <!-- Detect Live -->
              <svg v-if="stage.id === 'detect'" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <circle cx="12" cy="12" r="2" />
                <path d="M16.24 7.76a6 6 0 0 1 0 8.49" />
                <path d="M7.76 16.24a6 6 0 0 1 0-8.49" />
                <path d="M19.07 4.93a10 10 0 0 1 0 14.14" />
                <path d="M4.93 19.07a10 10 0 0 1 0-14.14" />
              </svg>
              <!-- Record & Repair -->
              <svg v-else-if="stage.id === 'record'" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <circle cx="12" cy="12" r="9" />
                <circle cx="12" cy="12" r="3" fill="currentColor" />
              </svg>
              <!-- Post-Process DAG -->
              <svg v-else-if="stage.id === 'postprocess'" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <rect x="3" y="4" width="6" height="6" rx="1.5" />
                <rect x="15" y="4" width="6" height="6" rx="1.5" />
                <rect x="15" y="14" width="6" height="6" rx="1.5" />
                <path d="M9 7h6M12 7v10h3" />
              </svg>
              <!-- Notify & Replay -->
              <svg v-else width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <path d="M18 8A6 6 0 0 0 6 8c0 7-3 9-3 9h18s-3-2-3-9" />
                <path d="M13.73 21a2 2 0 0 1-3.46 0" />
              </svg>
            </span>
          </span>
          <span class="flow-node-title">{{ isZh ? stage.titleZh : stage.titleEn }}</span>
          <span class="flow-node-short">{{ isZh ? stage.shortZh : stage.shortEn }}</span>
          <span
            v-if="idx === activeIndex && isPlaying"
            :key="`${activeIndex}-${progressSeed}`"
            class="flow-node-progress"
            aria-hidden="true"
          />
        </button>

        <div
          v-if="idx < stages.length - 1"
          class="flow-connector"
          :class="{ 'is-lit': idx < activeIndex, 'is-flowing': idx === activeIndex - 1 || (activeIndex === 0 && idx === 0 && isPlaying) }"
          aria-hidden="true"
        >
          <svg viewBox="0 0 44 16" fill="none" class="flow-connector-svg">
            <line x1="2" y1="8" x2="38" y2="8" class="flow-line-base" />
            <line x1="2" y1="8" x2="38" y2="8" class="flow-line-pulse" />
            <path d="M34 4l5 4-5 4" class="flow-arrow" />
          </svg>
        </div>
      </template>
    </div>

    <div class="flow-detail" role="tabpanel">
      <div class="flow-detail-main">
        <div class="flow-detail-title-row">
          <span class="flow-detail-step">{{ activeStage.step }}</span>
          <h3 class="flow-detail-title">
            {{ isZh ? activeStage.titleZh : activeStage.titleEn }}
          </h3>
          <span class="flow-status-pill">
            <span class="flow-status-dot" />
            {{ isZh ? activeStage.statusZh : activeStage.statusEn }}
          </span>
        </div>
        <p class="flow-detail-desc">
          {{ isZh ? activeStage.descZh : activeStage.descEn }}
        </p>
        <div class="flow-tags">
          <span
            v-for="tag in isZh ? activeStage.highlightsZh : activeStage.highlightsEn"
            :key="tag"
            class="flow-tag"
          >
            {{ tag }}
          </span>
        </div>
      </div>

      <!-- Stage-specific animated micro-visual preview -->
      <div class="flow-visual-stage" role="presentation" aria-hidden="true">
        <Transition name="flow-stage-fade" mode="out-in">
          <!-- Stage 0: Detect Live -->
          <div v-if="activeStage.id === 'detect'" key="detect" class="stage-card stage-detect">
            <div class="radar-box">
              <span class="radar-ring radar-ring-outer" />
              <span class="radar-ring radar-ring-inner" />
              <span class="radar-sweep" />
              <span class="radar-core" />
            </div>
            <div class="poll-list">
              <div
                v-for="row in pollRows"
                :key="row.platform"
                class="poll-row"
                :class="{ 'is-live': row.live }"
              >
                <span class="poll-left">
                  <span
                    class="poll-dot"
                    :class="{ 'is-glowing': row.live }"
                    :style="{ '--poll-accent': row.live ? row.accent : 'var(--vp-c-text-3)' }"
                  />
                  <span class="poll-name">{{ row.platform }}</span>
                </span>
                <span class="poll-sep">·</span>
                <span class="poll-state" :class="{ 'is-live': row.live }">{{ row.status }}</span>
              </div>
            </div>
          </div>

          <!-- Stage 1: Record & Repair -->
          <div v-else-if="activeStage.id === 'record'" key="record" class="stage-card stage-record">
            <div class="record-top">
              <span class="rec-badge">
                <span class="rec-dot" />
                REC · 1080p60
              </span>
              <span class="pts-pill">
                <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round">
                  <polyline points="20 6 9 17 4 12" />
                </svg>
                PTS 00:00 -> 100% OK
              </span>
            </div>
            <div class="record-viz">
              <div class="eq-bars">
                <span
                  v-for="(h, i) in equalizerHeights"
                  :key="i"
                  class="eq-bar"
                  :style="{
                    '--bar-h': `${h}%`,
                    animationDelay: `${i * 0.11}s`,
                  }"
                />
              </div>
              <div class="danmu-lane">
                <span class="danmu-chip danmu-chip-a">
                  {{ isZh ? '弹幕 · 66666 稳！' : 'Danmu · 66666 crisp!' }}
                </span>
                <span class="danmu-chip danmu-chip-b">
                  {{ isZh ? '高能预警 · 同步写入 XML' : 'Sync · XML captured' }}
                </span>
              </div>
            </div>
          </div>

          <!-- Stage 2: Post-Process DAG -->
          <div v-else-if="activeStage.id === 'postprocess'" key="postprocess" class="stage-card stage-dag">
            <div class="dag-header">
              <span class="dag-label">{{ isZh ? 'DAG 执行管线' : 'DAG EXECUTION' }}</span>
              <span class="dag-ok">3 / 3 OK</span>
            </div>
            <svg viewBox="0 0 320 76" fill="none" class="dag-svg">
              <!-- Animated dashed edges -->
              <line x1="88" y1="38" x2="118" y2="38" class="dag-edge-base" />
              <line x1="88" y1="38" x2="118" y2="38" class="dag-edge-pulse" />
              <path d="M113 34l5 4-5 4" class="dag-edge-arrow" />

              <line x1="202" y1="38" x2="232" y2="38" class="dag-edge-base" />
              <line x1="202" y1="38" x2="232" y2="38" class="dag-edge-pulse dag-edge-pulse-2" />
              <path d="M227 34l5 4-5 4" class="dag-edge-arrow" />

              <!-- Node 1: Remux -->
              <g class="dag-node">
                <rect x="6" y="16" width="82" height="44" rx="10" class="dag-node-bg" />
                <circle cx="22" cy="38" r="4" class="dag-node-dot-1" />
                <text x="33" y="36" class="dag-node-title">Remux</text>
                <text x="33" y="49" class="dag-node-sub">.mp4</text>
              </g>

              <!-- Node 2: Upload -->
              <g class="dag-node">
                <rect x="120" y="16" width="82" height="44" rx="10" class="dag-node-bg dag-node-bg-mid" />
                <circle cx="136" cy="38" r="4" class="dag-node-dot-2" />
                <text x="147" y="36" class="dag-node-title">Upload</text>
                <text x="147" y="49" class="dag-node-sub">rclone</text>
              </g>

              <!-- Node 3: Clean -->
              <g class="dag-node">
                <rect x="234" y="16" width="80" height="44" rx="10" class="dag-node-bg" />
                <circle cx="250" cy="38" r="4" class="dag-node-dot-3" />
                <text x="261" y="36" class="dag-node-title">Clean</text>
                <text x="261" y="49" class="dag-node-sub">safe</text>
              </g>
            </svg>
          </div>

          <!-- Stage 3: Notify & Replay -->
          <div v-else key="notify" class="stage-card stage-notify">
            <div class="notify-toast">
              <span class="notify-icon">
                <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round">
                  <path d="M22 2 11 13" />
                  <path d="m22 2-7 20-4-9-9-4 20-7z" />
                </svg>
              </span>
              <div class="notify-copy">
                <span class="notify-title">Telegram · Recording Archived</span>
                <span class="notify-sub">
                  {{ isZh ? '1080p60 · 14,820 条弹幕已同步' : '1080p60 · 14,820 chat messages synced' }}
                </span>
              </div>
              <span class="notify-time">now</span>
            </div>

            <div class="replay-bar">
              <span class="replay-play">
                <svg width="10" height="10" viewBox="0 0 24 24" fill="currentColor">
                  <path d="M8 5v14l11-7z" />
                </svg>
              </span>
              <div class="replay-track-wrap">
                <div class="replay-track">
                  <div class="replay-fill" />
                  <span class="replay-knob" />
                </div>
                <div class="replay-ticks">
                  <span style="left: 18%; height: 6px" />
                  <span style="left: 34%; height: 10px" />
                  <span style="left: 52%; height: 12px" />
                  <span style="left: 68%; height: 8px" />
                  <span style="left: 84%; height: 6px" />
                </div>
              </div>
              <span class="replay-time">01:42 / 02:15</span>
            </div>
          </div>
        </Transition>
      </div>
    </div>
  </div>
</template>

<style scoped>
.flow-demo {
  border: 1px solid var(--vp-c-divider);
  border-radius: 16px;
  background: var(--vp-c-bg-soft);
  padding: 24px;
  margin: 16px 0;
}

.flow-demo.is-compact {
  padding: 18px;
}

.flow-header {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  margin-bottom: 20px;
}

.flow-heading {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.flow-eyebrow {
  font-size: 11px;
  font-weight: 700;
  letter-spacing: 0.08em;
  text-transform: uppercase;
  color: var(--vp-c-brand-1);
}

.flow-caption {
  font-size: 13px;
  color: var(--vp-c-text-2);
}

.flow-toggle {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 6px 12px;
  border-radius: 999px;
  border: 1px solid var(--vp-c-divider);
  background: var(--vp-c-bg);
  color: var(--vp-c-text-1);
  font-size: 12px;
  font-weight: 600;
  cursor: pointer;
  transition: border-color 0.2s ease, color 0.2s ease, background-color 0.2s ease;
}

.flow-toggle:hover {
  border-color: var(--vp-c-brand-1);
  color: var(--vp-c-brand-1);
}

.flow-pipeline {
  display: grid;
  grid-template-columns: 1fr auto 1fr auto 1fr auto 1fr;
  align-items: center;
  gap: 6px;
}

.flow-node {
  position: relative;
  overflow: hidden;
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  text-align: left;
  gap: 4px;
  padding: 14px;
  border-radius: 12px;
  border: 1px solid var(--vp-c-divider);
  background: var(--vp-c-bg);
  cursor: pointer;
  transition:
    border-color 0.2s ease,
    box-shadow 0.2s ease,
    transform 0.2s ease,
    background-color 0.2s ease;
}

.flow-node:hover {
  border-color: var(--vp-c-brand-1);
  transform: translateY(-1px);
}

.flow-node.is-active {
  border-color: var(--vp-c-brand-1);
  background: var(--vp-c-bg-elv);
  box-shadow: 0 0 0 3px var(--vp-c-brand-soft);
}

.flow-node-progress {
  position: absolute;
  left: 0;
  right: 0;
  bottom: 0;
  height: 3px;
  background: linear-gradient(90deg, var(--vp-c-brand-1) 0%, #f16529 55%, #8b5cf6 100%);
  transform-origin: left center;
  transform: scaleX(0);
  animation: flow-progress 4s linear forwards;
  pointer-events: none;
}

@keyframes flow-progress {
  from {
    transform: scaleX(0);
  }
  to {
    transform: scaleX(1);
  }
}

.flow-node-top {
  width: 100%;
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 4px;
}

.flow-step {
  font-family: var(--vp-font-family-mono);
  font-size: 11px;
  font-weight: 700;
  color: var(--vp-c-text-3);
}

.flow-node.is-active .flow-step,
.flow-node.is-complete .flow-step {
  color: var(--vp-c-brand-1);
}

.flow-icon {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 30px;
  height: 30px;
  border-radius: 8px;
  background: var(--vp-c-bg-soft);
  color: var(--vp-c-text-2);
  transition: background-color 0.2s ease, color 0.2s ease;
}

.flow-node.is-active .flow-icon {
  background: var(--vp-c-brand-soft);
  color: var(--vp-c-brand-1);
}

.flow-node-title {
  font-size: 14px;
  font-weight: 600;
  color: var(--vp-c-text-1);
  line-height: 1.3;
}

.flow-node-short {
  font-size: 12px;
  color: var(--vp-c-text-2);
  line-height: 1.3;
}

.flow-connector {
  width: 36px;
  display: flex;
  align-items: center;
  justify-content: center;
}

.flow-connector-svg {
  width: 36px;
  height: 16px;
}

.flow-line-base {
  stroke: var(--vp-c-divider);
  stroke-width: 2;
  stroke-linecap: round;
}

.flow-line-pulse {
  stroke: var(--vp-c-brand-1);
  stroke-width: 2;
  stroke-linecap: round;
  stroke-dasharray: 6 10;
  opacity: 0.25;
}

.flow-arrow {
  stroke: var(--vp-c-text-3);
  stroke-width: 1.75;
  stroke-linecap: round;
  stroke-linejoin: round;
}

.flow-connector.is-lit .flow-line-pulse,
.flow-connector.is-flowing .flow-line-pulse {
  opacity: 1;
  animation: flow-dash 1.2s linear infinite;
}

.flow-connector.is-lit .flow-arrow,
.flow-connector.is-flowing .flow-arrow {
  stroke: var(--vp-c-brand-1);
}

@keyframes flow-dash {
  to {
    stroke-dashoffset: -32;
  }
}

.flow-detail {
  margin-top: 18px;
  padding: 18px 20px;
  border-radius: 12px;
  border: 1px solid var(--vp-c-divider);
  background: var(--vp-c-bg);
  display: grid;
  grid-template-columns: minmax(0, 1.15fr) minmax(0, 0.85fr);
  gap: 20px;
  align-items: center;
}

.flow-demo.is-compact .flow-detail {
  margin-top: 14px;
  padding: 14px 16px;
  gap: 16px;
}

.flow-detail-main {
  min-width: 0;
}

.flow-detail-title-row {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px 10px;
  margin-bottom: 10px;
}

.flow-detail-step {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  padding: 2px 8px;
  border-radius: 999px;
  background: var(--vp-c-brand-soft);
  color: var(--vp-c-brand-1);
  font-family: var(--vp-font-family-mono);
  font-size: 12px;
  font-weight: 700;
}

.flow-detail-title {
  margin: 0 !important;
  font-size: 16px !important;
  font-weight: 600;
  color: var(--vp-c-text-1);
  border: none !important;
  padding: 0 !important;
}

.flow-status-pill {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 3px 10px;
  border-radius: 999px;
  border: 1px solid var(--vp-c-divider);
  background: var(--vp-c-bg-soft);
  color: var(--vp-c-text-2);
  font-family: var(--vp-font-family-mono);
  font-size: 11px;
  max-width: 100%;
}

.flow-status-dot {
  flex-shrink: 0;
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: var(--vp-c-brand-1);
  animation: status-pulse 2s ease-in-out infinite;
}

@keyframes status-pulse {
  0%,
  100% {
    opacity: 1;
    transform: scale(1);
  }
  50% {
    opacity: 0.45;
    transform: scale(0.85);
  }
}

.flow-detail-desc {
  margin: 0 0 12px !important;
  font-size: 14px;
  line-height: 1.65;
  color: var(--vp-c-text-2);
}

.flow-tags {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
}

.flow-tag {
  display: inline-flex;
  align-items: center;
  padding: 3px 10px;
  border-radius: 6px;
  background: var(--vp-c-bg-soft);
  border: 1px solid var(--vp-c-divider);
  font-size: 12px;
  font-weight: 500;
  color: var(--vp-c-text-1);
}

/* Right-hand live micro-visual stage card */
.flow-visual-stage {
  min-width: 0;
  border-radius: 12px;
  border: 1px solid var(--vp-c-divider);
  background:
    radial-gradient(120% 100% at 95% 0%, rgba(124, 58, 237, 0.08), transparent 62%),
    radial-gradient(100% 90% at 5% 100%, rgba(228, 77, 38, 0.08), transparent 62%),
    var(--vp-c-bg-soft);
  padding: 14px;
  min-height: 132px;
  display: flex;
  align-items: center;
  justify-content: center;
  overflow: hidden;
}

:global(.dark) .flow-visual-stage {
  background:
    radial-gradient(120% 100% at 95% 0%, rgba(139, 92, 246, 0.14), transparent 62%),
    radial-gradient(100% 90% at 5% 100%, rgba(241, 101, 41, 0.12), transparent 62%),
    var(--vp-c-bg-alt);
}

.stage-card {
  width: 100%;
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.flow-stage-fade-enter-active,
.flow-stage-fade-leave-active {
  transition: opacity 0.2s ease, transform 0.2s ease;
}

.flow-stage-fade-enter-from {
  opacity: 0;
  transform: translateY(4px);
}

.flow-stage-fade-leave-to {
  opacity: 0;
  transform: translateY(-4px);
}

/* Stage 0: Detect radar & channel poll rows */
.stage-detect {
  flex-direction: row;
  align-items: center;
  gap: 14px;
}

.radar-box {
  position: relative;
  flex-shrink: 0;
  width: 68px;
  height: 68px;
  border-radius: 50%;
  border: 1px solid color-mix(in srgb, var(--vp-c-brand-1) 30%, var(--vp-c-divider));
  background: var(--vp-c-bg);
  display: flex;
  align-items: center;
  justify-content: center;
  overflow: hidden;
}

.radar-ring {
  position: absolute;
  border-radius: 50%;
  border: 1px solid color-mix(in srgb, var(--vp-c-brand-1) 24%, transparent);
}

.radar-ring-outer {
  width: 50px;
  height: 50px;
  animation: radar-ping 2.4s ease-out infinite;
}

.radar-ring-inner {
  width: 28px;
  height: 28px;
}

.radar-sweep {
  position: absolute;
  inset: 0;
  border-radius: 50%;
  background: conic-gradient(
    from 0deg,
    color-mix(in srgb, var(--vp-c-brand-1) 36%, transparent) 0deg,
    transparent 75deg,
    transparent 360deg
  );
  animation: radar-spin 2.8s linear infinite;
}

.radar-core {
  position: relative;
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--vp-c-brand-1);
  box-shadow: 0 0 0 4px var(--vp-c-brand-soft);
}

@keyframes radar-spin {
  to {
    transform: rotate(360deg);
  }
}

@keyframes radar-ping {
  0% {
    transform: scale(0.75);
    opacity: 0.75;
  }
  100% {
    transform: scale(1.22);
    opacity: 0;
  }
}

.poll-list {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.poll-row {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 5px 9px;
  border-radius: 8px;
  border: 1px solid var(--vp-c-divider);
  background: var(--vp-c-bg);
  font-family: var(--vp-font-family-mono);
  font-size: 11px;
  color: var(--vp-c-text-2);
}

.poll-row.is-live {
  border-color: color-mix(in srgb, var(--vp-c-brand-1) 28%, var(--vp-c-divider));
}

.poll-left {
  display: inline-flex;
  align-items: center;
  gap: 7px;
  min-width: 0;
}

.poll-dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: var(--poll-accent);
  flex-shrink: 0;
}

.poll-dot.is-glowing {
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--poll-accent) 26%, transparent);
  animation: status-pulse 1.8s ease-in-out infinite;
}

.poll-name {
  font-weight: 600;
  color: var(--vp-c-text-1);
}

.poll-sep {
  color: var(--vp-c-text-3);
}

.poll-state {
  margin-left: auto;
  font-size: 10px;
  font-weight: 700;
  letter-spacing: 0.04em;
  color: var(--vp-c-text-3);
}

.poll-state.is-live {
  color: #16a34a;
}

:global(.dark) .poll-state.is-live {
  color: #4ade80;
}

/* Stage 1: Record equalizer + PTS repair + drifting Danmu */
.record-top {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  justify-content: space-between;
  gap: 6px;
}

.rec-badge {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  font-family: var(--vp-font-family-mono);
  font-size: 10px;
  font-weight: 700;
  color: var(--vp-c-text-1);
}

.rec-dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: #ef4444;
  box-shadow: 0 0 0 3px rgba(239, 68, 68, 0.22);
  animation: status-pulse 1.4s ease-in-out infinite;
}

.pts-pill {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 2px 8px;
  border-radius: 999px;
  background: rgba(22, 163, 74, 0.12);
  border: 1px solid rgba(22, 163, 74, 0.28);
  color: #15803d;
  font-family: var(--vp-font-family-mono);
  font-size: 10px;
  font-weight: 700;
}

:global(.dark) .pts-pill {
  background: rgba(74, 222, 128, 0.14);
  border-color: rgba(74, 222, 128, 0.28);
  color: #4ade80;
}

.record-viz {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.eq-bars {
  height: 40px;
  padding: 6px 10px;
  border-radius: 8px;
  border: 1px solid var(--vp-c-divider);
  background: var(--vp-c-bg);
  display: flex;
  align-items: flex-end;
  justify-content: space-between;
  gap: 5px;
}

.eq-bar {
  flex: 1;
  height: var(--bar-h);
  border-radius: 999px;
  background: linear-gradient(180deg, #8b5cf6 0%, var(--vp-c-brand-1) 100%);
  transform-origin: bottom center;
  animation: eq-bounce 1.2s ease-in-out infinite alternate;
}

@keyframes eq-bounce {
  0% {
    transform: scaleY(0.38);
    opacity: 0.72;
  }
  100% {
    transform: scaleY(1);
    opacity: 1;
  }
}

.danmu-lane {
  position: relative;
  height: 24px;
  border-radius: 7px;
  border: 1px solid var(--vp-c-divider);
  background: var(--vp-c-bg);
  overflow: hidden;
}

.danmu-chip {
  position: absolute;
  top: 3px;
  white-space: nowrap;
  padding: 1px 8px;
  border-radius: 999px;
  font-size: 10px;
  font-weight: 600;
  line-height: 1.5;
}

.danmu-chip-a {
  background: var(--vp-c-brand-soft);
  color: var(--vp-c-brand-1);
  animation: danmu-drift 5.5s linear infinite;
}

.danmu-chip-b {
  background: rgba(124, 58, 237, 0.12);
  color: #7c3aed;
  animation: danmu-drift 5.5s linear infinite;
  animation-delay: -2.75s;
}

:global(.dark) .danmu-chip-b {
  background: rgba(167, 139, 250, 0.16);
  color: #c4b5fd;
}

@keyframes danmu-drift {
  0% {
    left: 100%;
    transform: translateX(0);
  }
  100% {
    left: 0;
    transform: translateX(-110%);
  }
}

/* Stage 2: Compact 3-node SVG DAG mini-flow */
.stage-dag {
  gap: 6px;
}

.dag-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  font-family: var(--vp-font-family-mono);
  font-size: 10px;
}

.dag-label {
  font-weight: 700;
  letter-spacing: 0.06em;
  color: var(--vp-c-text-3);
}

.dag-ok {
  font-weight: 700;
  color: var(--vp-c-brand-1);
}

.dag-svg {
  width: 100%;
  height: auto;
  display: block;
}

.dag-edge-base {
  stroke: var(--vp-c-divider);
  stroke-width: 2;
  stroke-linecap: round;
}

.dag-edge-pulse {
  stroke: var(--vp-c-brand-1);
  stroke-width: 2;
  stroke-linecap: round;
  stroke-dasharray: 5 7;
  animation: flow-dash 1.1s linear infinite;
}

.dag-edge-pulse-2 {
  stroke: #8b5cf6;
  animation-delay: -0.35s;
}

.dag-edge-arrow {
  stroke: var(--vp-c-brand-1);
  stroke-width: 1.75;
  stroke-linecap: round;
  stroke-linejoin: round;
}

.dag-node-bg {
  fill: var(--vp-c-bg);
  stroke: var(--vp-c-divider);
  stroke-width: 1.25;
}

.dag-node-bg-mid {
  stroke: color-mix(in srgb, var(--vp-c-brand-1) 50%, var(--vp-c-divider));
}

.dag-node-dot-1 {
  fill: var(--vp-c-brand-1);
}

.dag-node-dot-2 {
  fill: #8b5cf6;
}

.dag-node-dot-3 {
  fill: #22c55e;
}

.dag-node-title {
  fill: var(--vp-c-text-1);
  font-size: 11px;
  font-weight: 700;
  font-family: var(--vp-font-family-base);
}

.dag-node-sub {
  fill: var(--vp-c-text-3);
  font-size: 9.5px;
  font-family: var(--vp-font-family-mono);
}

/* Stage 3: Frosted alert toast + seekable timeline */
.stage-notify {
  gap: 10px;
}

.notify-toast {
  display: flex;
  align-items: center;
  gap: 9px;
  padding: 9px 11px;
  border-radius: 10px;
  border: 1px solid color-mix(in srgb, var(--vp-c-brand-1) 26%, var(--vp-c-divider));
  background: color-mix(in srgb, var(--vp-c-bg) 86%, transparent);
  backdrop-filter: blur(10px);
  box-shadow: 0 8px 20px -14px rgba(0, 0, 0, 0.25);
}

.notify-icon {
  flex-shrink: 0;
  width: 26px;
  height: 26px;
  border-radius: 8px;
  background: linear-gradient(135deg, var(--vp-c-brand-soft), rgba(124, 58, 237, 0.14));
  color: var(--vp-c-brand-1);
  display: inline-flex;
  align-items: center;
  justify-content: center;
}

.notify-copy {
  min-width: 0;
  flex: 1;
  display: flex;
  flex-direction: column;
  gap: 1px;
}

.notify-title {
  font-size: 11.5px;
  font-weight: 700;
  color: var(--vp-c-text-1);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.notify-sub {
  font-size: 10.5px;
  color: var(--vp-c-text-2);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.notify-time {
  font-family: var(--vp-font-family-mono);
  font-size: 10px;
  color: var(--vp-c-text-3);
}

.replay-bar {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 10px;
  border-radius: 9px;
  border: 1px solid var(--vp-c-divider);
  background: var(--vp-c-bg);
}

.replay-play {
  flex-shrink: 0;
  width: 22px;
  height: 22px;
  border-radius: 50%;
  background: var(--vp-c-brand-soft);
  color: var(--vp-c-brand-1);
  display: inline-flex;
  align-items: center;
  justify-content: center;
}

.replay-track-wrap {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.replay-track {
  position: relative;
  height: 5px;
  border-radius: 999px;
  background: var(--vp-c-divider);
}

.replay-fill {
  height: 100%;
  width: 68%;
  border-radius: 999px;
  background: linear-gradient(90deg, var(--vp-c-brand-1) 0%, #8b5cf6 100%);
  animation: timeline-seek 4s ease-in-out infinite alternate;
}

.replay-knob {
  position: absolute;
  top: 50%;
  left: 68%;
  width: 10px;
  height: 10px;
  border-radius: 50%;
  background: var(--vp-c-bg);
  border: 2px solid var(--vp-c-brand-1);
  transform: translate(-50%, -50%);
  box-shadow: 0 0 0 3px var(--vp-c-brand-soft);
  animation: knob-seek 4s ease-in-out infinite alternate;
}

@keyframes timeline-seek {
  from {
    width: 48%;
  }
  to {
    width: 78%;
  }
}

@keyframes knob-seek {
  from {
    left: 48%;
  }
  to {
    left: 78%;
  }
}

.replay-ticks {
  position: relative;
  height: 12px;
}

.replay-ticks span {
  position: absolute;
  bottom: 0;
  width: 3px;
  border-radius: 999px;
  background: color-mix(in srgb, var(--vp-c-brand-1) 45%, var(--vp-c-divider));
}

.replay-time {
  flex-shrink: 0;
  font-family: var(--vp-font-family-mono);
  font-size: 10px;
  color: var(--vp-c-text-3);
}

@media (max-width: 820px) {
  .flow-pipeline {
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 10px;
  }

  .flow-connector {
    display: none;
  }
}

@media (max-width: 720px) {
  .flow-detail {
    grid-template-columns: 1fr;
    gap: 14px;
  }
}

@media (max-width: 480px) {
  .flow-pipeline {
    grid-template-columns: 1fr;
  }

  .stage-detect {
    flex-direction: column;
    align-items: stretch;
  }

  .radar-box {
    align-self: center;
  }
}

@media (prefers-reduced-motion: reduce) {
  .flow-node-progress {
    display: none;
    animation: none;
  }

  .flow-line-pulse,
  .flow-status-dot,
  .radar-ring-outer,
  .radar-sweep,
  .poll-dot.is-glowing,
  .rec-dot,
  .eq-bar,
  .danmu-chip,
  .dag-edge-pulse,
  .replay-fill,
  .replay-knob {
    animation: none;
  }

  .danmu-chip-a {
    left: 8px;
    transform: none;
  }

  .danmu-chip-b {
    display: none;
  }

  .flow-node,
  .flow-stage-fade-enter-active,
  .flow-stage-fade-leave-active {
    transition: none;
    transform: none;
  }
}
</style>
