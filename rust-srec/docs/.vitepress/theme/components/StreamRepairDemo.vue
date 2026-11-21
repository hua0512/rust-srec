<script setup lang="ts">
import { computed, ref } from 'vue'
import { useData } from 'vitepress'

const { lang } = useData()
const isZh = computed(() => lang.value.startsWith('zh'))

const repairEnabled = ref(true)

const rawSegments = computed(() => [
  {
    id: 's1',
    ts: '00:00 → 12:04',
    status: 'ok' as const,
    labelEn: 'Normal stream',
    labelZh: '正常流数据',
  },
  {
    id: 's2',
    ts: '47:51:09 (!)',
    status: 'error' as const,
    labelEn: 'Timestamp jump',
    labelZh: '时间戳跳变',
  },
  {
    id: 's3',
    ts: 'Stalled 18s',
    status: 'warn' as const,
    labelEn: 'Frozen frame',
    labelZh: '画面卡死',
  },
  {
    id: 's4',
    ts: '00:00 (reset)',
    status: 'error' as const,
    labelEn: 'Broken seek index',
    labelZh: '进度条无法拖动',
  },
])

const repairedSegments = computed(() => [
  {
    id: 'r1',
    ts: '00:00 → 30:00',
    status: 'ok' as const,
    labelEn: 'Rebased & synced',
    labelZh: '时间戳归零对齐',
  },
  {
    id: 'r2',
    ts: '30:00 → 45:00',
    status: 'ok' as const,
    labelEn: 'Stall recovered',
    labelZh: '自动恢复卡顿',
  },
  {
    id: 'r3',
    ts: '45:00 → 60:00',
    status: 'ok' as const,
    labelEn: 'Clean seekable part',
    labelZh: '分段无缝可拖动',
  },
])
</script>

<template>
  <div class="repair-demo">
    <div class="repair-header">
      <div class="repair-heading">
        <span class="repair-eyebrow">
          {{ isZh ? '实时流修复对比' : 'LIVE STREAM REPAIR COMPARISON' }}
        </span>
        <span class="repair-subtitle">
          {{
            isZh
              ? '切换对比原始录制流与开启 Mesio 实时修复后的播放体验'
              : 'Toggle between an unrepaired raw broadcast and Mesio live stream repair'
          }}
        </span>
      </div>

      <div class="repair-switch" role="tablist" :aria-label="isZh ? '流修复模式' : 'Stream repair mode'">
        <button
          type="button"
          role="tab"
          class="switch-btn"
          :class="{ 'is-active is-raw': !repairEnabled }"
          :aria-selected="!repairEnabled"
          @click="repairEnabled = false"
        >
          {{ isZh ? '未修复原始流' : 'Unrepaired Raw Stream' }}
        </button>
        <button
          type="button"
          role="tab"
          class="switch-btn"
          :class="{ 'is-active is-repaired': repairEnabled }"
          :aria-selected="repairEnabled"
          @click="repairEnabled = true"
        >
          {{ isZh ? '启用 Mesio 实时修复' : 'With Mesio Live Repair' }}
        </button>
      </div>
    </div>

    <div class="repair-stage" :class="repairEnabled ? 'mode-repaired' : 'mode-raw'">
      <div class="timeline-bar">
        <div class="timeline-head">
          <span class="timeline-label">
            {{
              repairEnabled
                ? isZh
                  ? '输出时间轴（00:00 → 30:00 → 60:00）'
                  : 'Output Timeline (00:00 → 30:00 → 60:00)'
                : isZh
                  ? '原始流时间轴（存在跳变与中断）'
                  : 'Raw Stream Timeline (Jumps & Stalls)'
            }}
          </span>
          <span class="timeline-pill" :class="repairEnabled ? 'pill-ok' : 'pill-err'">
            {{
              repairEnabled
                ? isZh
                  ? '可正常拖动 · 时长准确'
                  : 'Seekable · Accurate Duration'
                : isZh
                  ? '进度条错乱 · 播放器卡顿'
                  : 'Broken Seek · Player Stalls'
            }}
          </span>
        </div>

        <div class="seekbar-track" aria-hidden="true">
          <div class="seekbar-fill" />
          <div v-if="!repairEnabled" class="seekbar-glitch g1" />
          <div v-if="!repairEnabled" class="seekbar-glitch g2" />
          <div class="seekbar-head" />
        </div>
      </div>

      <div class="segments-grid">
        <div
          v-for="seg in repairEnabled ? repairedSegments : rawSegments"
          :key="seg.id"
          class="seg-card"
          :class="`seg-${seg.status}`"
        >
          <span class="seg-ts">{{ seg.ts }}</span>
          <span class="seg-label">{{ isZh ? seg.labelZh : seg.labelEn }}</span>
        </div>
      </div>

      <div class="repair-metrics">
        <div class="metric-box">
          <span class="metric-key">{{ isZh ? '时间戳处理' : 'Timestamps' }}</span>
          <span class="metric-val">
            {{
              repairEnabled
                ? isZh
                  ? '自动归零与连续重排'
                  : 'Rebased & monotonically aligned'
                : isZh
                  ? '保留上游跳变与负偏移'
                  : 'Upstream jumps & resets kept'
            }}
          </span>
        </div>
        <div class="metric-box">
          <span class="metric-key">{{ isZh ? '编解码切换 / 卡顿' : 'Codec Change / Stalls' }}</span>
          <span class="metric-val">
            {{
              repairEnabled
                ? isZh
                  ? '检测参数变化自动切分新片段'
                  : 'Auto-splits clean segments on change'
                : isZh
                  ? '花屏、音画不同步或静默挂起'
                  : 'Corrupt frames, A/V drift, or hangs'
            }}
          </span>
        </div>
        <div class="metric-box">
          <span class="metric-key">{{ isZh ? '播放器兼容性' : 'Player Playback' }}</span>
          <span class="metric-val">
            {{
              repairEnabled
                ? isZh
                  ? 'VLC / mpv / 浏览器直接拖动播放'
                  : 'Opens & seeks cleanly in any player'
                : isZh
                  ? '时长显示异常，拖动直接跳回开头'
                  : 'Wrong duration; seeking breaks playback'
            }}
          </span>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.repair-demo {
  border: 1px solid var(--vp-c-divider);
  border-radius: 16px;
  background: var(--vp-c-bg-soft);
  padding: 22px;
  margin: 20px 0;
}

.repair-header {
  display: flex;
  flex-wrap: wrap;
  align-items: flex-start;
  justify-content: space-between;
  gap: 12px;
  margin-bottom: 16px;
}

.repair-heading {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.repair-eyebrow {
  font-size: 11px;
  font-weight: 700;
  letter-spacing: 0.08em;
  color: var(--vp-c-brand-1);
}

.repair-subtitle {
  font-size: 13px;
  color: var(--vp-c-text-2);
}

.repair-switch {
  display: inline-flex;
  flex-wrap: wrap;
  gap: 6px;
  padding: 4px;
  border-radius: 999px;
  border: 1px solid var(--vp-c-divider);
  background: var(--vp-c-bg);
}

.switch-btn {
  padding: 6px 12px;
  border-radius: 999px;
  border: none;
  background: transparent;
  color: var(--vp-c-text-2);
  font-size: 12px;
  font-weight: 600;
  cursor: pointer;
  transition: background-color 0.2s ease, color 0.2s ease;
}

.switch-btn.is-active.is-repaired {
  background: var(--vp-c-success-soft);
  color: var(--vp-c-success-1);
}

.switch-btn.is-active.is-raw {
  background: var(--vp-c-danger-soft);
  color: var(--vp-c-danger-1);
}

.repair-stage {
  display: flex;
  flex-direction: column;
  gap: 14px;
  padding: 18px;
  border-radius: 12px;
  border: 1px solid var(--vp-c-divider);
  background: var(--vp-c-bg);
}

.timeline-head {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  margin-bottom: 10px;
}

.timeline-label {
  font-size: 13px;
  font-weight: 600;
  color: var(--vp-c-text-1);
}

.timeline-pill {
  padding: 2px 10px;
  border-radius: 999px;
  font-size: 11px;
  font-weight: 600;
}

.pill-ok {
  background: var(--vp-c-success-soft);
  color: var(--vp-c-success-1);
}

.pill-err {
  background: var(--vp-c-danger-soft);
  color: var(--vp-c-danger-1);
}

.seekbar-track {
  position: relative;
  height: 10px;
  border-radius: 999px;
  background: var(--vp-c-bg-soft);
  border: 1px solid var(--vp-c-divider);
  overflow: hidden;
}

.seekbar-fill {
  position: absolute;
  inset: 0 auto 0 0;
  border-radius: 999px;
}

.mode-repaired .seekbar-fill {
  width: 100%;
  background: linear-gradient(90deg, var(--vp-c-green-2), var(--vp-c-brand-1));
  animation: seek-smooth 4.5s linear infinite;
}

.mode-raw .seekbar-fill {
  width: 65%;
  background: var(--vp-c-red-2);
  opacity: 0.55;
  animation: seek-jitter 2.6s steps(4, end) infinite;
}

.seekbar-glitch {
  position: absolute;
  top: 0;
  bottom: 0;
  width: 10%;
  background: var(--vp-c-bg);
  opacity: 0.9;
}

.seekbar-glitch.g1 {
  left: 34%;
}

.seekbar-glitch.g2 {
  left: 58%;
}

.seekbar-head {
  position: absolute;
  top: 1px;
  bottom: 1px;
  width: 8px;
  border-radius: 999px;
  background: #ffffff;
  box-shadow: 0 0 6px rgba(0, 0, 0, 0.35);
}

.mode-repaired .seekbar-head {
  animation: head-smooth 4.5s linear infinite;
}

.mode-raw .seekbar-head {
  animation: head-jitter 2.6s steps(5, end) infinite;
}

@keyframes seek-smooth {
  0% {
    width: 6%;
  }
  100% {
    width: 100%;
  }
}

@keyframes head-smooth {
  0% {
    left: 4%;
  }
  100% {
    left: calc(100% - 10px);
  }
}

@keyframes seek-jitter {
  0% {
    width: 28%;
  }
  40% {
    width: 72%;
  }
  65% {
    width: 40%;
  }
  100% {
    width: 18%;
  }
}

@keyframes head-jitter {
  0% {
    left: 24%;
  }
  40% {
    left: 68%;
  }
  65% {
    left: 36%;
  }
  100% {
    left: 12%;
  }
}

.segments-grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(140px, 1fr));
  gap: 10px;
}

.seg-card {
  display: flex;
  flex-direction: column;
  gap: 3px;
  padding: 10px 12px;
  border-radius: 10px;
  border: 1px solid var(--vp-c-divider);
  background: var(--vp-c-bg-soft);
}

.seg-ok {
  border-color: var(--vp-c-success-soft);
}

.seg-warn {
  border-color: var(--vp-c-warning-2);
  background: var(--vp-c-warning-soft);
}

.seg-error {
  border-color: var(--vp-c-danger-2);
  background: var(--vp-c-danger-soft);
}

.seg-ts {
  font-family: var(--vp-font-family-mono);
  font-size: 12px;
  font-weight: 700;
  color: var(--vp-c-text-1);
}

.seg-label {
  font-size: 12px;
  color: var(--vp-c-text-2);
}

.repair-metrics {
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  gap: 10px;
}

.metric-box {
  display: flex;
  flex-direction: column;
  gap: 4px;
  padding: 10px 12px;
  border-radius: 10px;
  border: 1px solid var(--vp-c-divider);
  background: var(--vp-c-bg-soft);
}

.metric-key {
  font-size: 11px;
  font-weight: 700;
  text-transform: uppercase;
  letter-spacing: 0.04em;
  color: var(--vp-c-text-3);
}

.metric-val {
  font-size: 13px;
  line-height: 1.45;
  color: var(--vp-c-text-1);
}

@media (max-width: 720px) {
  .repair-metrics {
    grid-template-columns: 1fr;
  }
}

@media (prefers-reduced-motion: reduce) {
  .seekbar-fill,
  .seekbar-head {
    animation: none !important;
  }

  .mode-repaired .seekbar-fill {
    width: 100%;
  }

  .mode-repaired .seekbar-head {
    left: calc(100% - 10px);
  }

  .mode-raw .seekbar-fill {
    width: 48%;
  }

  .mode-raw .seekbar-head {
    left: 44%;
  }
}
</style>
