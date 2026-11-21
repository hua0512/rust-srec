<script setup lang="ts">
import { computed, reactive, ref } from 'vue'
import { useData } from 'vitepress'

const { lang } = useData()
const isZh = computed(() => lang.value.startsWith('zh'))

type LayerKey = 'global' | 'platform' | 'template' | 'streamer'

interface Scenario {
  id: string
  labelEn: string
  labelZh: string
  values: Record<LayerKey, { en: string; zh: string }>
  defaultEnabled: Record<'platform' | 'template' | 'streamer', boolean>
}

const scenarios: Scenario[] = [
  {
    id: 'format',
    labelEn: 'Output Format',
    labelZh: '输出格式',
    values: {
      global: { en: 'flv (global default)', zh: 'flv（全局默认）' },
      platform: { en: 'ts (HLS platform)', zh: 'ts（HLS 平台设置）' },
      template: { en: 'mp4 (4K Archive template)', zh: 'mp4（高清归档模板）' },
      streamer: { en: 'mkv (custom streamer override)', zh: 'mkv（主播单独覆盖）' },
    },
    defaultEnabled: { platform: true, template: true, streamer: false },
  },
  {
    id: 'danmu',
    labelEn: 'Danmu Recording',
    labelZh: '弹幕录制',
    values: {
      global: { en: 'Disabled', zh: '关闭' },
      platform: { en: 'Enabled (XML chat log)', zh: '开启（保存 XML 弹幕）' },
      template: { en: 'Enabled + Statistics', zh: '开启并生成弹幕统计' },
      streamer: { en: 'Disabled (video only)', zh: '关闭（仅录视频）' },
    },
    defaultEnabled: { platform: true, template: false, streamer: false },
  },
  {
    id: 'workflow',
    labelEn: 'Upload Workflow / Proxy',
    labelZh: '上传工作流 / 代理',
    values: {
      global: { en: 'Direct · Keep local files', zh: '直连 · 仅保留本地文件' },
      platform: { en: 'Use HTTP Proxy (region access)', zh: '启用 HTTP 代理（跨区访问）' },
      template: { en: 'Proxy + Remux -> rclone upload', zh: '代理 + 封装 -> rclone 上传' },
      streamer: { en: 'Remux -> Local NAS only', zh: '仅封装 -> 保留到本地 NAS' },
    },
    defaultEnabled: { platform: true, template: true, streamer: true },
  },
]

const selectedScenarioId = ref(scenarios[0].id)

const overrides = reactive<Record<'platform' | 'template' | 'streamer', boolean>>({
  ...scenarios[0].defaultEnabled,
})

const currentScenario = computed(
  () => scenarios.find((s) => s.id === selectedScenarioId.value) ?? scenarios[0],
)

function pickScenario(scenario: Scenario) {
  selectedScenarioId.value = scenario.id
  overrides.platform = scenario.defaultEnabled.platform
  overrides.template = scenario.defaultEnabled.template
  overrides.streamer = scenario.defaultEnabled.streamer
}

function toggleLayer(layer: 'platform' | 'template' | 'streamer') {
  overrides[layer] = !overrides[layer]
}

interface LayerDef {
  key: LayerKey
  nameEn: string
  nameZh: string
  priority: number
  toggleable: boolean
}

const layerDefs: LayerDef[] = [
  { key: 'global', nameEn: 'Global', nameZh: '全局设置', priority: 1, toggleable: false },
  { key: 'platform', nameEn: 'Platform', nameZh: '平台配置', priority: 2, toggleable: true },
  { key: 'template', nameEn: 'Template', nameZh: '模板配置', priority: 3, toggleable: true },
  { key: 'streamer', nameEn: 'Streamer', nameZh: '主播设置', priority: 4, toggleable: true },
]

function isLayerEnabled(key: LayerKey): boolean {
  if (key === 'global') return true
  return overrides[key]
}

const winningLayer = computed<LayerKey>(() => {
  if (overrides.streamer) return 'streamer'
  if (overrides.template) return 'template'
  if (overrides.platform) return 'platform'
  return 'global'
})

const winningLayerDef = computed(
  () => layerDefs.find((l) => l.key === winningLayer.value) ?? layerDefs[0],
)

const effectiveValue = computed(() => {
  const val = currentScenario.value.values[winningLayer.value]
  return isZh.value ? val.zh : val.en
})
</script>

<template>
  <div class="layers-demo">
    <div class="layers-top">
      <div class="layers-title-group">
        <span class="layers-eyebrow">
          {{ isZh ? '4 层配置继承模拟器' : '4-LAYER INHERITANCE SIMULATOR' }}
        </span>
        <span class="layers-subtitle">
          {{
            isZh
              ? '点击平台、模板或主播层切换是否覆盖，观察最终生效配置如何逐层解析'
              : 'Toggle overrides on Platform, Template, or Streamer to see which layer wins'
          }}
        </span>
      </div>
      <div class="scenario-pills" role="tablist" :aria-label="isZh ? '配置场景' : 'Configuration scenarios'">
        <button
          v-for="scenario in scenarios"
          :key="scenario.id"
          type="button"
          role="tab"
          class="scenario-pill"
          :class="{ 'is-active': scenario.id === selectedScenarioId }"
          :aria-selected="scenario.id === selectedScenarioId"
          @click="pickScenario(scenario)"
        >
          {{ isZh ? scenario.labelZh : scenario.labelEn }}
        </button>
      </div>
    </div>

    <div class="layers-body">
      <div class="layers-stack">
        <template v-for="(layer, idx) in layerDefs" :key="layer.key">
          <button
            type="button"
            class="layer-row"
            :class="{
              'is-enabled': isLayerEnabled(layer.key),
              'is-winner': winningLayer === layer.key,
              'is-base': !layer.toggleable,
            }"
            :disabled="!layer.toggleable"
            :aria-pressed="layer.toggleable ? isLayerEnabled(layer.key) : undefined"
            @click="layer.toggleable && toggleLayer(layer.key as 'platform' | 'template' | 'streamer')"
          >
            <div class="layer-left">
              <span class="layer-index">{{ layer.priority }}</span>
              <div class="layer-meta">
                <span class="layer-name">
                  {{ isZh ? layer.nameZh : layer.nameEn }}
                </span>
                <span class="layer-value">
                  {{
                    isLayerEnabled(layer.key)
                      ? isZh
                        ? currentScenario.values[layer.key].zh
                        : currentScenario.values[layer.key].en
                      : isZh
                        ? '未覆盖（沿用上层）'
                        : 'Not set (inherits above)'
                  }}
                </span>
              </div>
            </div>

            <div class="layer-right">
              <span v-if="!layer.toggleable" class="layer-badge badge-base">
                {{ isZh ? '基础兜底' : 'Base' }}
              </span>
              <span
                v-else-if="isLayerEnabled(layer.key)"
                class="layer-badge"
                :class="winningLayer === layer.key ? 'badge-winner' : 'badge-override'"
              >
                {{
                  winningLayer === layer.key
                    ? isZh
                      ? '最终生效'
                      : 'Active Override'
                    : isZh
                      ? '已覆盖'
                      : 'Overridden'
                }}
              </span>
              <span v-else class="layer-badge badge-inherited">
                {{ isZh ? '继承上层' : 'Inherited' }}
              </span>
            </div>
          </button>

          <div
            v-if="idx < layerDefs.length - 1"
            class="layer-beam"
            :class="{ 'is-active-beam': idx < layerDefs.findIndex((l) => l.key === winningLayer) }"
            aria-hidden="true"
          >
            <span class="beam-track">
              <span class="beam-pulse" />
            </span>
          </div>
        </template>
      </div>

      <div class="effective-card">
        <div class="effective-header">
          <span class="effective-label">
            {{ isZh ? '最终生效值' : 'EFFECTIVE VALUE' }}
          </span>
          <span class="effective-source">
            {{ isZh ? '来源：' : 'From: ' }}
            <strong>{{ isZh ? winningLayerDef.nameZh : winningLayerDef.nameEn }}</strong>
          </span>
        </div>
        <div class="effective-value">{{ effectiveValue }}</div>
        <p class="effective-note">
          {{
            isZh
              ? '优先级从低到高：全局 → 平台 → 模板 → 主播。未设置的字段自动向下继承，修改一次模板即可批量更新关联主播。'
              : 'Priority runs lowest to highest: Global → Platform → Template → Streamer. Unset fields cascade downward automatically.'
          }}
        </p>
      </div>
    </div>
  </div>
</template>

<style scoped>
.layers-demo {
  border: 1px solid var(--vp-c-divider);
  border-radius: 16px;
  background: var(--vp-c-bg-soft);
  padding: 22px;
  margin: 20px 0;
}

.layers-top {
  display: flex;
  flex-wrap: wrap;
  align-items: flex-start;
  justify-content: space-between;
  gap: 14px;
  margin-bottom: 18px;
}

.layers-title-group {
  display: flex;
  flex-direction: column;
  gap: 3px;
}

.layers-eyebrow {
  font-size: 11px;
  font-weight: 700;
  letter-spacing: 0.08em;
  text-transform: uppercase;
  color: var(--vp-c-brand-1);
}

.layers-subtitle {
  font-size: 13px;
  color: var(--vp-c-text-2);
}

.scenario-pills {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
}

.scenario-pill {
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

.scenario-pill:hover {
  border-color: var(--vp-c-brand-1);
  color: var(--vp-c-text-1);
}

.scenario-pill.is-active {
  border-color: var(--vp-c-brand-1);
  background: var(--vp-c-brand-soft);
  color: var(--vp-c-brand-1);
  font-weight: 600;
}

.layers-body {
  display: grid;
  grid-template-columns: minmax(0, 1.4fr) minmax(0, 1fr);
  gap: 18px;
  align-items: stretch;
}

.layers-stack {
  display: flex;
  flex-direction: column;
}

.layer-row {
  width: 100%;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 12px 14px;
  border-radius: 12px;
  border: 1px solid var(--vp-c-divider);
  background: var(--vp-c-bg);
  text-align: left;
  cursor: pointer;
  transition:
    border-color 0.2s ease,
    box-shadow 0.2s ease,
    opacity 0.2s ease,
    background-color 0.2s ease;
}

.layer-row.is-base {
  cursor: default;
}

.layer-row:not(.is-base):hover {
  border-color: var(--vp-c-brand-1);
}

.layer-row:not(.is-enabled) {
  opacity: 0.72;
  border-style: dashed;
}

.layer-row.is-winner {
  border-color: var(--vp-c-brand-1);
  background: var(--vp-c-bg-elv);
  box-shadow: 0 0 0 3px var(--vp-c-brand-soft);
}

.layer-left {
  display: flex;
  align-items: center;
  gap: 12px;
  min-width: 0;
}

.layer-index {
  width: 24px;
  height: 24px;
  border-radius: 50%;
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  font-family: var(--vp-font-family-mono);
  font-size: 12px;
  font-weight: 700;
  background: var(--vp-c-bg-soft);
  color: var(--vp-c-text-2);
}

.layer-row.is-winner .layer-index {
  background: var(--vp-c-brand-soft);
  color: var(--vp-c-brand-1);
}

.layer-meta {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}

.layer-name {
  font-size: 14px;
  font-weight: 600;
  color: var(--vp-c-text-1);
}

.layer-value {
  font-size: 12px;
  color: var(--vp-c-text-2);
  font-family: var(--vp-font-family-mono);
}

.layer-right {
  flex-shrink: 0;
}

.layer-badge {
  display: inline-flex;
  align-items: center;
  padding: 3px 9px;
  border-radius: 999px;
  font-size: 11px;
  font-weight: 600;
  white-space: nowrap;
}

.badge-base {
  background: var(--vp-c-default-soft);
  color: var(--vp-c-text-2);
}

.badge-inherited {
  background: var(--vp-c-bg-soft);
  border: 1px solid var(--vp-c-divider);
  color: var(--vp-c-text-3);
}

.badge-override {
  background: var(--vp-c-default-soft);
  color: var(--vp-c-text-1);
}

.badge-winner {
  background: var(--vp-c-brand-soft);
  color: var(--vp-c-brand-1);
}

.layer-beam {
  height: 18px;
  display: flex;
  justify-content: flex-start;
  padding-left: 25px;
}

.beam-track {
  position: relative;
  width: 2px;
  height: 100%;
  background: var(--vp-c-divider);
  overflow: hidden;
}

.beam-pulse {
  display: block;
  width: 100%;
  height: 10px;
  background: var(--vp-c-brand-1);
  opacity: 0;
}

.layer-beam.is-active-beam .beam-track {
  background: var(--vp-c-brand-soft);
}

.layer-beam.is-active-beam .beam-pulse {
  opacity: 1;
  animation: beam-flow 1.2s ease-in-out infinite;
}

@keyframes beam-flow {
  0% {
    transform: translateY(-10px);
  }
  100% {
    transform: translateY(18px);
  }
}

.effective-card {
  display: flex;
  flex-direction: column;
  justify-content: center;
  gap: 12px;
  padding: 20px;
  border-radius: 14px;
  border: 1px solid var(--vp-c-brand-1);
  background: var(--vp-c-bg);
}

.effective-header {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
}

.effective-label {
  font-size: 11px;
  font-weight: 700;
  letter-spacing: 0.08em;
  color: var(--vp-c-brand-1);
}

.effective-source {
  font-size: 12px;
  color: var(--vp-c-text-2);
}

.effective-value {
  padding: 12px 14px;
  border-radius: 10px;
  background: var(--vp-c-brand-soft);
  color: var(--vp-c-text-1);
  font-family: var(--vp-font-family-mono);
  font-size: 14px;
  font-weight: 600;
}

.effective-note {
  margin: 0 !important;
  font-size: 13px;
  line-height: 1.6;
  color: var(--vp-c-text-2);
}

@media (max-width: 768px) {
  .layers-body {
    grid-template-columns: 1fr;
  }
}

@media (prefers-reduced-motion: reduce) {
  .beam-pulse {
    animation: none;
  }
}
</style>
