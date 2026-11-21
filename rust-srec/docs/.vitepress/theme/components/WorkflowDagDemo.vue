<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { useData } from 'vitepress'

const { lang } = useData()
const isZh = computed(() => lang.value.startsWith('zh'))

type PresetId = 'archive' | 'upload' | 'failure'
type NodeStatus = 'pending' | 'running' | 'done' | 'failed' | 'skipped'

interface DagNode {
  id: string
  x: number
  y: number
  titleEn: string
  titleZh: string
  subEn: string
  subZh: string
  stage: number
}

interface DagEdge {
  id: string
  from: string
  to: string
  path: string
}

interface PresetConfig {
  id: PresetId
  labelEn: string
  labelZh: string
  summaryEn: string
  summaryZh: string
  nodes: DagNode[]
  edges: DagEdge[]
  maxStage: number
  finalStatus: Record<string, NodeStatus>
}

const presets: PresetConfig[] = [
  {
    id: 'archive',
    labelEn: 'Archive Locally',
    labelZh: '本地归档',
    summaryEn:
      'Remux and thumbnail generation run in parallel after recording ends, then the finished files are moved to your archive folder.',
    summaryZh:
      '录制完成后并行执行封装与封面生成，两步全部完成后将成品移入本地归档目录。',
    maxStage: 3,
    nodes: [
      { id: 'rec', x: 20, y: 78, titleEn: 'Recording', titleZh: '录制完成', subEn: 'raw.flv', subZh: 'raw.flv', stage: 0 },
      { id: 'remux', x: 215, y: 24, titleEn: 'Remux MP4', titleZh: '封装 MP4', subEn: 'Lossless copy', subZh: '无损重封装', stage: 1 },
      { id: 'thumb', x: 215, y: 132, titleEn: 'Thumbnail', titleZh: '生成封面', subEn: 'Preview grid', subZh: '缩略图预览', stage: 1 },
      { id: 'move', x: 430, y: 78, titleEn: 'Move to NAS', titleZh: '移入归档', subEn: '/archive/...', subZh: '/archive/...', stage: 2 },
    ],
    edges: [
      { id: 'e1', from: 'rec', to: 'remux', path: 'M 164 108 C 190 108, 190 54, 215 54' },
      { id: 'e2', from: 'rec', to: 'thumb', path: 'M 164 108 C 190 108, 190 162, 215 162' },
      { id: 'e3', from: 'remux', to: 'move', path: 'M 359 54 C 392 54, 392 108, 430 108' },
      { id: 'e4', from: 'thumb', to: 'move', path: 'M 359 162 C 392 162, 392 108, 430 108' },
    ],
    finalStatus: { rec: 'done', remux: 'done', thumb: 'done', move: 'done' },
  },
  {
    id: 'upload',
    labelEn: 'Upload & Clean Up',
    labelZh: '上传并清理',
    summaryEn:
      'After remuxing and thumbnail creation succeed, rclone uploads the outputs to cloud storage and only then removes local temp files.',
    summaryZh:
      '封装与封面生成完成后通过 rclone 上传至云盘，确认上传成功后再自动清理本地临时文件。',
    maxStage: 4,
    nodes: [
      { id: 'rec', x: 12, y: 78, titleEn: 'Recording', titleZh: '录制完成', subEn: 'segment.flv', subZh: 'segment.flv', stage: 0 },
      { id: 'remux', x: 176, y: 24, titleEn: 'Remux MP4', titleZh: '封装 MP4', subEn: 'Faststart', subZh: 'Faststart', stage: 1 },
      { id: 'thumb', x: 176, y: 132, titleEn: 'Thumbnail', titleZh: '生成封面', subEn: 'cover.jpg', subZh: 'cover.jpg', stage: 1 },
      { id: 'upload', x: 356, y: 78, titleEn: 'rclone Upload', titleZh: 'rclone 上传', subEn: 'Cloud drive', subZh: '云端存储', stage: 2 },
      { id: 'delete', x: 526, y: 78, titleEn: 'Delete Local', titleZh: '清理本地', subEn: 'Free disk', subZh: '释放空间', stage: 3 },
    ],
    edges: [
      { id: 'e1', from: 'rec', to: 'remux', path: 'M 142 108 C 160 108, 158 54, 176 54' },
      { id: 'e2', from: 'rec', to: 'thumb', path: 'M 142 108 C 160 108, 158 162, 176 162' },
      { id: 'e3', from: 'remux', to: 'upload', path: 'M 306 54 C 330 54, 330 108, 356 108' },
      { id: 'e4', from: 'thumb', to: 'upload', path: 'M 306 162 C 330 162, 330 108, 356 108' },
      { id: 'e5', from: 'upload', to: 'delete', path: 'M 486 108 L 526 108' },
    ],
    finalStatus: { rec: 'done', remux: 'done', thumb: 'done', upload: 'done', delete: 'done' },
  },
  {
    id: 'failure',
    labelEn: 'Failure Safety',
    labelZh: '失败保护',
    summaryEn:
      'If cloud upload fails due to a network outage or expired token, downstream Delete Local is automatically skipped so your recording is never lost.',
    summaryZh:
      '当云盘上传因网络波动或凭据过期失败时，下游「清理本地」节点会自动跳过，确保录像文件绝不被误删。',
    maxStage: 4,
    nodes: [
      { id: 'rec', x: 12, y: 78, titleEn: 'Recording', titleZh: '录制完成', subEn: 'segment.flv', subZh: 'segment.flv', stage: 0 },
      { id: 'remux', x: 176, y: 24, titleEn: 'Remux MP4', titleZh: '封装 MP4', subEn: 'Faststart', subZh: 'Faststart', stage: 1 },
      { id: 'thumb', x: 176, y: 132, titleEn: 'Thumbnail', titleZh: '生成封面', subEn: 'cover.jpg', subZh: 'cover.jpg', stage: 1 },
      { id: 'upload', x: 356, y: 78, titleEn: 'rclone Upload', titleZh: 'rclone 上传', subEn: 'Network error', subZh: '网络超时失败', stage: 2 },
      { id: 'delete', x: 526, y: 78, titleEn: 'Delete Local', titleZh: '清理本地', subEn: 'File preserved', subZh: '已保留原文件', stage: 3 },
    ],
    edges: [
      { id: 'e1', from: 'rec', to: 'remux', path: 'M 142 108 C 160 108, 158 54, 176 54' },
      { id: 'e2', from: 'rec', to: 'thumb', path: 'M 142 108 C 160 108, 158 162, 176 162' },
      { id: 'e3', from: 'remux', to: 'upload', path: 'M 306 54 C 330 54, 330 108, 356 108' },
      { id: 'e4', from: 'thumb', to: 'upload', path: 'M 306 162 C 330 162, 330 108, 356 108' },
      { id: 'e5', from: 'upload', to: 'delete', path: 'M 486 108 L 526 108' },
    ],
    finalStatus: { rec: 'done', remux: 'done', thumb: 'done', upload: 'failed', delete: 'skipped' },
  },
]

const selectedPresetId = ref<PresetId>('upload')
const stepCursor = ref(4)
let timer: ReturnType<typeof setInterval> | null = null

const currentPreset = computed(
  () => presets.find((p) => p.id === selectedPresetId.value) ?? presets[1],
)

const nodeWidth = computed(() => (currentPreset.value.nodes.length <= 4 ? 144 : 130))
const viewBoxWidth = computed(() => (currentPreset.value.nodes.length <= 4 ? 600 : 670))

function getNodeStatus(node: DagNode): NodeStatus {
  if (stepCursor.value < node.stage) return 'pending'
  if (stepCursor.value === node.stage) {
    return currentPreset.value.finalStatus[node.id] === 'skipped' ? 'skipped' : 'running'
  }
  return currentPreset.value.finalStatus[node.id] ?? 'done'
}

function getEdgeState(edge: DagEdge): 'idle' | 'active' | 'done' | 'blocked' {
  const fromNode = currentPreset.value.nodes.find((n) => n.id === edge.from)
  const toNode = currentPreset.value.nodes.find((n) => n.id === edge.to)
  if (!fromNode || !toNode) return 'idle'
  const fromStatus = getNodeStatus(fromNode)
  const toStatus = getNodeStatus(toNode)
  if (fromStatus === 'failed' || toStatus === 'skipped') return 'blocked'
  if (toStatus === 'running') return 'active'
  if (toStatus === 'done' || toStatus === 'failed') return 'done'
  return 'idle'
}

function statusLabel(status: NodeStatus): string {
  if (isZh.value) {
    switch (status) {
      case 'pending':
        return '等待中'
      case 'running':
        return '运行中'
      case 'done':
        return '已完成'
      case 'failed':
        return '失败'
      case 'skipped':
        return '已跳过（安全）'
    }
  }
  switch (status) {
    case 'pending':
      return 'Pending'
    case 'running':
      return 'Running'
    case 'done':
      return 'Done'
    case 'failed':
      return 'Failed'
    case 'skipped':
      return 'Skipped (Safe)'
  }
}

function stopTimer() {
  if (timer !== null) {
    clearInterval(timer)
    timer = null
  }
}

function runSimulation() {
  stopTimer()
  stepCursor.value = 0
  timer = setInterval(() => {
    if (stepCursor.value >= currentPreset.value.maxStage) {
      stopTimer()
      return
    }
    stepCursor.value += 1
  }, 850)
}

function selectPreset(id: PresetId) {
  selectedPresetId.value = id
  const prefersReduced =
    typeof window !== 'undefined' &&
    window.matchMedia('(prefers-reduced-motion: reduce)').matches
  if (prefersReduced) {
    stopTimer()
    stepCursor.value = currentPreset.value.maxStage
  } else {
    runSimulation()
  }
}

onMounted(() => {
  const prefersReduced = window.matchMedia('(prefers-reduced-motion: reduce)').matches
  if (prefersReduced) {
    stepCursor.value = currentPreset.value.maxStage
  } else {
    runSimulation()
  }
})

onUnmounted(() => {
  stopTimer()
})
</script>

<template>
  <div class="dag-demo">
    <div class="dag-header">
      <div class="dag-heading">
        <span class="dag-eyebrow">
          {{ isZh ? '后处理工作流 DAG 演示' : 'WORKFLOW DAG SIMULATOR' }}
        </span>
        <span class="dag-subtitle">
          {{
            isZh
              ? '切换预设查看并行任务执行与失败保护机制'
              : 'Switch presets to inspect parallel execution and failure-safe cleanup'
          }}
        </span>
      </div>
      <div class="dag-controls">
        <div class="dag-presets" role="tablist" :aria-label="isZh ? '工作流预设' : 'Workflow presets'">
          <button
            v-for="preset in presets"
            :key="preset.id"
            type="button"
            role="tab"
            class="dag-preset-btn"
            :class="{ 'is-active': preset.id === selectedPresetId }"
            :aria-selected="preset.id === selectedPresetId"
            @click="selectPreset(preset.id)"
          >
            {{ isZh ? preset.labelZh : preset.labelEn }}
          </button>
        </div>
        <button type="button" class="dag-replay-btn" @click="runSimulation">
          <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <polyline points="1 4 1 10 7 10" />
            <path d="M3.51 15a9 9 0 1 0 2.13-9.36L1 10" />
          </svg>
          <span>{{ isZh ? '重播' : 'Replay' }}</span>
        </button>
      </div>
    </div>

    <div class="dag-canvas-wrap">
      <svg
        class="dag-svg"
        :viewBox="`0 0 ${viewBoxWidth} 216`"
        role="img"
        :aria-label="isZh ? currentPreset.labelZh : currentPreset.labelEn"
      >
        <g>
          <path
            v-for="edge in currentPreset.edges"
            :key="edge.id"
            :d="edge.path"
            fill="none"
            class="dag-edge"
            :class="`edge-${getEdgeState(edge)}`"
          />
        </g>

        <g
          v-for="node in currentPreset.nodes"
          :key="node.id"
          :transform="`translate(${node.x}, ${node.y})`"
          class="dag-node-group"
          :class="`node-${getNodeStatus(node)}`"
        >
          <rect :width="nodeWidth" height="60" rx="10" class="dag-node-rect" />
          <circle cx="14" cy="18" r="4" class="dag-node-dot" />
          <text x="24" y="22" class="dag-node-title">
            {{ isZh ? node.titleZh : node.titleEn }}
          </text>
          <text x="12" y="39" class="dag-node-sub">
            {{ isZh ? node.subZh : node.subEn }}
          </text>
          <text x="12" y="53" class="dag-node-status">
            {{ statusLabel(getNodeStatus(node)) }}
          </text>
        </g>
      </svg>
    </div>

    <div class="dag-summary" :class="{ 'is-failure': selectedPresetId === 'failure' }">
      <p class="dag-summary-text">
        {{ isZh ? currentPreset.summaryZh : currentPreset.summaryEn }}
      </p>
    </div>
  </div>
</template>

<style scoped>
.dag-demo {
  border: 1px solid var(--vp-c-divider);
  border-radius: 16px;
  background: var(--vp-c-bg-soft);
  padding: 22px;
  margin: 20px 0;
}

.dag-header {
  display: flex;
  flex-wrap: wrap;
  align-items: flex-start;
  justify-content: space-between;
  gap: 12px;
  margin-bottom: 16px;
}

.dag-heading {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.dag-eyebrow {
  font-size: 11px;
  font-weight: 700;
  letter-spacing: 0.08em;
  color: var(--vp-c-brand-1);
}

.dag-subtitle {
  font-size: 13px;
  color: var(--vp-c-text-2);
}

.dag-controls {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px;
}

.dag-presets {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
}

.dag-preset-btn,
.dag-replay-btn {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  padding: 6px 12px;
  border-radius: 999px;
  border: 1px solid var(--vp-c-divider);
  background: var(--vp-c-bg);
  color: var(--vp-c-text-2);
  font-size: 12px;
  font-weight: 500;
  cursor: pointer;
  transition: border-color 0.2s ease, background-color 0.2s ease, color 0.2s ease;
}

.dag-preset-btn:hover,
.dag-replay-btn:hover {
  border-color: var(--vp-c-brand-1);
  color: var(--vp-c-text-1);
}

.dag-preset-btn.is-active {
  border-color: var(--vp-c-brand-1);
  background: var(--vp-c-brand-soft);
  color: var(--vp-c-brand-1);
  font-weight: 600;
}

.dag-canvas-wrap {
  border-radius: 12px;
  border: 1px solid var(--vp-c-divider);
  background: var(--vp-c-bg);
  padding: 12px 10px;
  overflow-x: auto;
}

.dag-svg {
  width: 100%;
  min-width: 540px;
  height: auto;
  display: block;
}

.dag-edge {
  stroke-width: 2.2;
  stroke-linecap: round;
  transition: stroke 0.25s ease;
}

.edge-idle {
  stroke: var(--vp-c-divider);
}

.edge-active {
  stroke: var(--vp-c-brand-1);
  stroke-dasharray: 6 6;
  animation: dag-flow 0.9s linear infinite;
}

.edge-done {
  stroke: var(--vp-c-green-2);
}

.edge-blocked {
  stroke: var(--vp-c-red-2);
  stroke-dasharray: 4 5;
  opacity: 0.75;
}

@keyframes dag-flow {
  to {
    stroke-dashoffset: -24;
  }
}

.dag-node-rect {
  fill: var(--vp-c-bg-soft);
  stroke: var(--vp-c-divider);
  stroke-width: 1.5;
  transition: stroke 0.25s ease, fill 0.25s ease;
}

.node-running .dag-node-rect {
  stroke: var(--vp-c-brand-1);
  fill: var(--vp-c-bg-elv);
}

.node-done .dag-node-rect {
  stroke: var(--vp-c-green-2);
}

.node-failed .dag-node-rect {
  stroke: var(--vp-c-red-2);
  fill: var(--vp-c-danger-soft);
}

.node-skipped .dag-node-rect {
  stroke: var(--vp-c-yellow-2);
  stroke-dasharray: 4 3;
}

.dag-node-dot {
  fill: var(--vp-c-text-3);
}

.node-running .dag-node-dot {
  fill: var(--vp-c-brand-1);
}

.node-done .dag-node-dot {
  fill: var(--vp-c-green-2);
}

.node-failed .dag-node-dot {
  fill: var(--vp-c-red-2);
}

.node-skipped .dag-node-dot {
  fill: var(--vp-c-yellow-2);
}

.dag-node-title {
  fill: var(--vp-c-text-1);
  font-size: 11.5px;
  font-weight: 700;
}

.dag-node-sub {
  fill: var(--vp-c-text-2);
  font-size: 10px;
  font-family: var(--vp-font-family-mono);
}

.dag-node-status {
  fill: var(--vp-c-text-3);
  font-size: 9.5px;
  font-weight: 600;
}

.node-running .dag-node-status {
  fill: var(--vp-c-brand-1);
}

.node-done .dag-node-status {
  fill: var(--vp-c-green-1);
}

.node-failed .dag-node-status {
  fill: var(--vp-c-red-1);
}

.node-skipped .dag-node-status {
  fill: var(--vp-c-yellow-1);
}

.dag-summary {
  margin-top: 14px;
  padding: 12px 14px;
  border-radius: 10px;
  border: 1px solid var(--vp-c-divider);
  background: var(--vp-c-bg);
}

.dag-summary.is-failure {
  border-color: var(--vp-c-warning-2);
  background: var(--vp-c-warning-soft);
}

.dag-summary-text {
  margin: 0 !important;
  font-size: 13px;
  line-height: 1.6;
  color: var(--vp-c-text-1);
}

@media (prefers-reduced-motion: reduce) {
  .edge-active {
    animation: none;
  }
}
</style>
