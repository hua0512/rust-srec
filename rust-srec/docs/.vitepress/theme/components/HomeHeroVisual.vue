<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { useData } from 'vitepress'

const { lang } = useData()
const isZh = computed(() => lang.value.startsWith('zh'))

interface ChannelPreset {
  id: 'bilibili' | 'twitch' | 'douyin'
  labelEn: string
  labelZh: string
  accent: string
  glowRgba: string
  handle: string
  titleEn: string
  titleZh: string
  streamMetric: string
  baseSeconds: number
  danmuEn: [string[], string[], string[]]
  danmuZh: [string[], string[], string[]]
}

const presets: ChannelPreset[] = [
  {
    id: 'bilibili',
    labelEn: 'Bilibili',
    labelZh: 'Bilibili',
    accent: '#fb7299',
    glowRgba: 'rgba(251, 114, 153, 0.24)',
    handle: '@sora_live · Room 21452505',
    titleEn: 'Late Night Synth & Acoustic Session',
    titleZh: '深夜合成器与原声吉他现场',
    streamMetric: '6.8 MB/s · 1080p60',
    baseSeconds: 6138, // 01:42:18
    danmuEn: [
      ['This synth transition is unreal', '60fps crystal clear tonight!', 'Recording automatically saved? Yes!', 'Encore please!!'],
      ['Best late-night stream on Bilibili', 'Chat is flying so fast', 'Audio mix sounds studio grade', 'GGs every time'],
      ['Drop the playlist after live!', '0 dropped frames on Mesio', 'Vibing from Tokyo', 'Bookmarking this moment'],
    ],
    danmuZh: [
      ['这段合成器转场太丝滑了', '今晚原画 60 帧清晰度拉满', '还好开着自动录制没错过', '安可再来一首！！'],
      ['深夜限定歌回氛围感绝了', '弹幕护体 哈哈哈哈', '混音质感跟录音棚一样', '高能预警 前方名场面'],
      ['求今晚歌单！', 'Mesio 实时修复 0 丢帧太稳了', '录播党狂喜 弹幕也全留住了', '这一段必须循环播放'],
    ],
  },
  {
    id: 'twitch',
    labelEn: 'Twitch',
    labelZh: 'Twitch',
    accent: '#a970ff',
    glowRgba: 'rgba(169, 112, 255, 0.26)',
    handle: 'twitch.tv/kairo_speedruns',
    titleEn: 'Any% World Record Attempts [AV1 1080p60]',
    titleZh: 'Any% 世界纪录冲刺挑战 [AV1 1080p60]',
    streamMetric: '5.4 MB/s · 1080p60',
    baseSeconds: 8104, // 02:15:04
    danmuEn: [
      ['-14.2s gold split!!', 'NO WAY THAT SKIP LANDED', 'Clip it!! Archive is safe', 'H.265 / AV1 looking crisp'],
      ['Chat hold your breath', 'Sub-40 pace is real tonight', 'Lets goooo!!', 'Cleanest movement I have seen'],
      ['Good luck on final boss', 'Never missing WR runs again', 'PB incoming!', 'Insane consistency'],
    ],
    danmuZh: [
      ['快了 14.2 秒！！红分了！', '这个极限穿墙竟然一次过了', '全自动录制已开启 安心看播', 'AV1 1080p60 画面好干净'],
      ['屏住呼吸 稳住别失误', '今晚有望冲进 40 分钟大关', '太强了 666666', '操作丝滑到离谱'],
      ['最终 Boss 加油！', '终于不用熬夜守着手动录了', '新纪录稳了！', '弹幕一起见证历史'],
    ],
  },
  {
    id: 'douyin',
    labelEn: 'Douyin',
    labelZh: '抖音',
    accent: '#fe2c55',
    glowRgba: 'rgba(254, 44, 85, 0.24)',
    handle: '@city_night_walk · Live',
    titleEn: 'Rainy Neon Street Photography Walk',
    titleZh: '雨夜霓虹街头扫街实况直播',
    streamMetric: '7.6 MB/s · 1440p60',
    baseSeconds: 3272, // 00:54:32
    danmuEn: [
      ['The rain reflections look cinematic', 'What lens are you shooting on?', '1440p60 bitrate is spotless', 'Walk past the neon arcade!'],
      ['So relaxing to watch after work', 'Love the ambient city rain audio', 'Auto-archived with full chat', 'Frame looks like a movie still'],
      ['Greetings from Shanghai', 'Zero stutter even on mobile link', 'Colors pop so nicely', 'Stay dry out there!'],
    ],
    danmuZh: [
      ['雨夜霓虹倒影太有电影感了', '主播用的什么定焦镜头呀？', '蓝光 1440p60 码率太顶了', '前面那条老街拍出来肯定好看'],
      ['下班看这个太解压了', '雨声和街头环境音好治愈', '录像和弹幕同步留档中', '随手一截都是壁纸'],
      ['打卡签到！画面真稳', '户外移动网络也没有卡顿花屏', '色彩氛围调得太棒了', '注意防雨别淋湿相机'],
    ],
  },
]

const activePresetId = ref<'bilibili' | 'twitch' | 'douyin'>('bilibili')
const elapsedTicks = ref(0)
let clockInterval: ReturnType<typeof setInterval> | null = null

const currentPreset = computed(
  () => presets.find((p) => p.id === activePresetId.value) ?? presets[0],
)

const formattedTimer = computed(() => {
  const total = currentPreset.value.baseSeconds + elapsedTicks.value
  const hours = Math.floor(total / 3600)
  const minutes = Math.floor((total % 3600) / 60)
  const seconds = total % 60
  return [hours, minutes, seconds].map((v) => String(v).padStart(2, '0')).join(':')
})

const activeTracks = computed(() =>
  isZh.value ? currentPreset.value.danmuZh : currentPreset.value.danmuEn,
)

const eqBars = [42, 74, 56, 88, 62, 95, 48, 78, 52, 84, 66, 44]

function selectPreset(id: 'bilibili' | 'twitch' | 'douyin') {
  activePresetId.value = id
}

onMounted(() => {
  const prefersReduced =
    typeof window !== 'undefined' &&
    window.matchMedia('(prefers-reduced-motion: reduce)').matches
  if (!prefersReduced) {
    clockInterval = setInterval(() => {
      elapsedTicks.value += 1
    }, 1000)
  }
})

onUnmounted(() => {
  if (clockInterval !== null) {
    clearInterval(clockInterval)
    clockInterval = null
  }
})
</script>

<template>
  <div
    class="studio-visual"
    :style="{
      '--preset-accent': currentPreset.accent,
      '--preset-glow': currentPreset.glowRgba,
    }"
  >
    <div class="studio-ambient" aria-hidden="true" />

    <div class="studio-card">
      <!-- Top Studio Control Bar -->
      <div class="studio-topbar">
        <div class="studio-topbar-left">
          <span class="rec-pill">
            <span class="rec-dot" aria-hidden="true" />
            <span>REC</span>
          </span>

          <div
            class="preset-group"
            role="tablist"
            :aria-label="isZh ? '切换示范直播平台' : 'Switch preview channel preset'"
          >
            <button
              v-for="preset in presets"
              :key="preset.id"
              type="button"
              role="tab"
              class="preset-chip"
              :class="{ 'is-active': preset.id === activePresetId }"
              :aria-selected="preset.id === activePresetId"
              @click="selectPreset(preset.id)"
            >
              <span
                class="preset-dot"
                :style="{ backgroundColor: preset.accent }"
                aria-hidden="true"
              />
              <span>{{ isZh ? preset.labelZh : preset.labelEn }}</span>
            </button>
          </div>
        </div>

        <div class="studio-timer" aria-live="off">
          <span class="timer-dot" aria-hidden="true" />
          <span>{{ formattedTimer }}</span>
        </div>
      </div>

      <!-- Broadcast Preview Canvas -->
      <div class="studio-stage">
        <div class="stage-bg-glow" aria-hidden="true" />
        <div class="stage-grid" aria-hidden="true" />

        <!-- Subtle Streamer Header Overlay -->
        <div class="stage-meta">
          <div class="stage-channel">
            <span class="stage-avatar" aria-hidden="true">
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <polygon points="6 4 20 12 6 20 6 4" />
              </svg>
            </span>
            <div class="stage-channel-text">
              <div class="stage-title">{{ isZh ? currentPreset.titleZh : currentPreset.titleEn }}</div>
              <div class="stage-handle">{{ currentPreset.handle }}</div>
            </div>
          </div>
          <span class="stage-live-tag">LIVE</span>
        </div>

        <!-- Subtle Center Broadcast Watermark & Equalizer -->
        <div class="stage-center" aria-hidden="true">
          <div class="stage-watermark">
            <svg width="26" height="26" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
              <rect x="2" y="5" width="15" height="14" rx="2.5" />
              <path d="m17 10 5-3v10l-5-3Z" />
            </svg>
          </div>
          <div class="stage-eq">
            <span
              v-for="(height, idx) in eqBars"
              :key="idx"
              class="eq-bar"
              :style="{
                '--bar-h': `${height}%`,
                animationDelay: `${idx * 0.11}s`,
              }"
            />
          </div>
        </div>

        <!-- 3 Staggered Floating Danmu Tracks -->
        <div class="danmu-layer" :key="`${activePresetId}-${isZh ? 'zh' : 'en'}`" aria-hidden="true">
          <div
            v-for="(track, trackIdx) in activeTracks"
            :key="trackIdx"
            class="danmu-track"
            :class="`danmu-track-${trackIdx}`"
          >
            <div class="danmu-belt">
              <span v-for="(msg, msgIdx) in track" :key="`a-${msgIdx}`" class="danmu-pill">
                {{ msg }}
              </span>
              <span v-for="(msg, msgIdx) in track" :key="`b-${msgIdx}`" class="danmu-pill">
                {{ msg }}
              </span>
            </div>
          </div>
        </div>
      </div>

      <!-- Bottom Telemetry Bar -->
      <div class="studio-telemetry">
        <div class="telemetry-pill telemetry-ok">
          <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z" />
            <path d="m9 12 2 2 4-4" />
          </svg>
          <span>{{ isZh ? 'Mesio 实时修复 · 0 丢帧' : 'Mesio Repair · 0 Dropped' }}</span>
        </div>

        <div class="telemetry-pill telemetry-mono">
          <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <polyline points="22 12 18 12 15 21 9 3 6 12 2 12" />
          </svg>
          <span>{{ currentPreset.streamMetric }}</span>
        </div>

        <div class="telemetry-pill telemetry-danmu">
          <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z" />
          </svg>
          <span>{{ isZh ? '弹幕同步中' : 'Danmu Synced' }}</span>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.studio-visual {
  position: relative;
  z-index: 1;
  width: 100%;
  max-width: 560px;
  text-align: left;
  color: #fafafa;
}

.studio-ambient {
  position: absolute;
  inset: -14px;
  border-radius: 26px;
  background:
    radial-gradient(60% 60% at 22% 24%, rgba(241, 101, 41, 0.34), transparent 72%),
    radial-gradient(58% 58% at 80% 78%, rgba(124, 58, 237, 0.3), transparent 72%);
  filter: blur(28px);
  opacity: 0.78;
  pointer-events: none;
  z-index: 0;
}

.studio-card {
  position: relative;
  z-index: 1;
  border-radius: 16px;
  border: 1px solid rgba(255, 255, 255, 0.11);
  background: linear-gradient(180deg, rgba(24, 24, 27, 0.96) 0%, rgba(17, 17, 20, 0.98) 100%);
  box-shadow:
    inset 0 1px 0 rgba(255, 255, 255, 0.09),
    0 24px 56px -18px rgba(0, 0, 0, 0.52),
    0 4px 16px -4px rgba(0, 0, 0, 0.32);
  overflow: hidden;
  backdrop-filter: blur(16px);
  -webkit-backdrop-filter: blur(16px);
}

/* Top bar */
.studio-topbar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding: 11px 14px;
  border-bottom: 1px solid rgba(255, 255, 255, 0.08);
  background: rgba(9, 9, 11, 0.6);
  flex-wrap: wrap;
}

.studio-topbar-left {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}

.rec-pill {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 3px 9px;
  border-radius: 9999px;
  background: rgba(239, 68, 68, 0.16);
  border: 1px solid rgba(239, 68, 68, 0.34);
  color: #fca5a5;
  font-family: var(--vp-font-family-mono);
  font-size: 11px;
  font-weight: 700;
  letter-spacing: 0.06em;
}

.rec-dot {
  width: 7px;
  height: 7px;
  border-radius: 9999px;
  background: #ef4444;
  box-shadow: 0 0 8px rgba(239, 68, 68, 0.9);
  animation: rec-blink 1.5s ease-in-out infinite;
}

.preset-group {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 2px;
  border-radius: 9px;
  background: rgba(255, 255, 255, 0.04);
  border: 1px solid rgba(255, 255, 255, 0.06);
}

.preset-chip {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  padding: 3px 9px;
  border-radius: 7px;
  border: 1px solid transparent;
  background: transparent;
  color: #a1a1aa;
  font-size: 11.5px;
  font-weight: 500;
  line-height: 1.35;
  cursor: pointer;
  transition:
    color 0.18s ease,
    background-color 0.18s ease,
    border-color 0.18s ease;
}

.preset-chip:hover {
  color: #fafafa;
}

.preset-chip.is-active {
  color: #fafafa;
  background: rgba(255, 255, 255, 0.1);
  border-color: rgba(255, 255, 255, 0.14);
  font-weight: 600;
}

.preset-chip:focus-visible {
  outline: 2px solid #f16529;
  outline-offset: 1px;
}

.preset-dot {
  width: 6px;
  height: 6px;
  border-radius: 9999px;
  flex-shrink: 0;
}

.studio-timer {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 3px 9px;
  border-radius: 7px;
  background: rgba(255, 255, 255, 0.05);
  border: 1px solid rgba(255, 255, 255, 0.08);
  font-family: var(--vp-font-family-mono);
  font-size: 12px;
  font-weight: 600;
  font-variant-numeric: tabular-nums;
  color: #e4e4e7;
}

.timer-dot {
  width: 5px;
  height: 5px;
  border-radius: 9999px;
  background: var(--preset-accent);
  box-shadow: 0 0 8px var(--preset-accent);
}

/* Broadcast preview stage */
.studio-stage {
  position: relative;
  height: 214px;
  overflow: hidden;
  background:
    radial-gradient(75% 95% at 50% 45%, rgba(39, 39, 42, 0.55) 0%, rgba(9, 9, 11, 0.94) 100%);
}

.stage-bg-glow {
  position: absolute;
  inset: 0;
  background:
    radial-gradient(55% 70% at 18% 20%, var(--preset-glow), transparent 72%),
    radial-gradient(55% 70% at 85% 82%, rgba(241, 101, 41, 0.18), transparent 72%);
  transition: background 0.35s ease;
  pointer-events: none;
}

.stage-grid {
  position: absolute;
  inset: 0;
  background-image: radial-gradient(rgba(255, 255, 255, 0.08) 1px, transparent 1px);
  background-size: 18px 18px;
  mask-image: linear-gradient(180deg, rgba(0, 0, 0, 0.55) 0%, transparent 100%);
  -webkit-mask-image: linear-gradient(180deg, rgba(0, 0, 0, 0.55) 0%, transparent 100%);
  pointer-events: none;
}

.stage-meta {
  position: relative;
  z-index: 3;
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 10px;
  padding: 13px 15px 0;
}

.stage-channel {
  display: flex;
  align-items: center;
  gap: 10px;
  min-width: 0;
}

.stage-avatar {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 30px;
  height: 30px;
  border-radius: 9px;
  background: rgba(255, 255, 255, 0.08);
  border: 1px solid rgba(255, 255, 255, 0.14);
  color: var(--preset-accent);
  flex-shrink: 0;
}

.stage-channel-text {
  min-width: 0;
}

.stage-title {
  font-size: 13px;
  font-weight: 600;
  line-height: 1.3;
  color: #fafafa;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.stage-handle {
  margin-top: 2px;
  font-family: var(--vp-font-family-mono);
  font-size: 11px;
  color: #a1a1aa;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.stage-live-tag {
  flex-shrink: 0;
  padding: 2px 7px;
  border-radius: 5px;
  background: rgba(255, 255, 255, 0.08);
  border: 1px solid rgba(255, 255, 255, 0.12);
  font-family: var(--vp-font-family-mono);
  font-size: 10px;
  font-weight: 700;
  letter-spacing: 0.08em;
  color: #d4d4d8;
}

/* Center subtle watermark & equalizer */
.stage-center {
  position: absolute;
  inset: 0;
  z-index: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 12px;
  padding-top: 26px;
  pointer-events: none;
}

.stage-watermark {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 52px;
  height: 52px;
  border-radius: 15px;
  background: rgba(255, 255, 255, 0.03);
  border: 1px solid rgba(255, 255, 255, 0.07);
  color: rgba(255, 255, 255, 0.24);
}

.stage-eq {
  display: flex;
  align-items: flex-end;
  gap: 4px;
  height: 24px;
  opacity: 0.42;
}

.eq-bar {
  width: 3px;
  height: var(--bar-h);
  border-radius: 9999px;
  background: linear-gradient(180deg, var(--preset-accent) 0%, #f16529 100%);
  transform-origin: bottom;
  animation: eq-bounce 1.35s ease-in-out infinite alternate;
}

/* Floating Danmu layer */
.danmu-layer {
  position: absolute;
  left: 0;
  right: 0;
  top: 58px;
  bottom: 14px;
  z-index: 2;
  display: flex;
  flex-direction: column;
  justify-content: space-evenly;
  overflow: hidden;
  pointer-events: none;
  mask-image: linear-gradient(90deg, transparent 0%, #000 8%, #000 92%, transparent 100%);
  -webkit-mask-image: linear-gradient(90deg, transparent 0%, #000 8%, #000 92%, transparent 100%);
}

.danmu-track {
  display: flex;
  width: 100%;
  overflow: hidden;
}

.danmu-belt {
  display: inline-flex;
  align-items: center;
  gap: 18px;
  padding-right: 18px;
  width: max-content;
  will-change: transform;
  animation: danmu-scroll 22s linear infinite;
}

.danmu-track-0 .danmu-belt {
  animation-duration: 24s;
  animation-delay: -5s;
}

.danmu-track-1 .danmu-belt {
  animation-duration: 19s;
  animation-delay: -11s;
}

.danmu-track-2 .danmu-belt {
  animation-duration: 27s;
  animation-delay: -17s;
}

.danmu-pill {
  display: inline-flex;
  align-items: center;
  padding: 4px 11px;
  border-radius: 9999px;
  background: rgba(24, 24, 27, 0.62);
  border: 1px solid rgba(255, 255, 255, 0.12);
  color: rgba(250, 250, 250, 0.9);
  font-size: 12px;
  font-weight: 500;
  line-height: 1.3;
  white-space: nowrap;
  backdrop-filter: blur(6px);
  -webkit-backdrop-filter: blur(6px);
  text-shadow: 0 1px 2px rgba(0, 0, 0, 0.5);
}

.danmu-track-1 .danmu-pill {
  background: rgba(24, 24, 27, 0.52);
  color: rgba(244, 244, 245, 0.84);
}

.danmu-track-2 .danmu-pill {
  background: rgba(24, 24, 27, 0.46);
  color: rgba(228, 228, 231, 0.78);
}

/* Bottom telemetry bar */
.studio-telemetry {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding: 11px 14px;
  border-top: 1px solid rgba(255, 255, 255, 0.08);
  background: rgba(9, 9, 11, 0.74);
}

.telemetry-pill {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 4px 9px;
  border-radius: 8px;
  background: rgba(255, 255, 255, 0.05);
  border: 1px solid rgba(255, 255, 255, 0.08);
  color: #d4d4d8;
  font-size: 11.5px;
  font-weight: 500;
  line-height: 1.3;
}

.telemetry-ok {
  color: #6ee7b7;
  border-color: rgba(16, 185, 129, 0.25);
  background: rgba(16, 185, 129, 0.1);
}

.telemetry-mono {
  font-family: var(--vp-font-family-mono);
  font-variant-numeric: tabular-nums;
  color: #fafafa;
}

.telemetry-danmu {
  color: #fdba74;
  border-color: rgba(241, 101, 41, 0.28);
  background: rgba(241, 101, 41, 0.1);
}

@keyframes rec-blink {
  0%,
  100% {
    opacity: 1;
  }
  50% {
    opacity: 0.35;
  }
}

@keyframes eq-bounce {
  0% {
    transform: scaleY(0.32);
  }
  100% {
    transform: scaleY(1);
  }
}

@keyframes danmu-scroll {
  from {
    transform: translate3d(0, 0, 0);
  }
  to {
    transform: translate3d(-50%, 0, 0);
  }
}

@media (max-width: 480px) {
  .studio-stage {
    height: 196px;
  }

  .studio-telemetry {
    justify-content: flex-start;
  }
}

@media (prefers-reduced-motion: reduce) {
  .rec-dot,
  .eq-bar,
  .danmu-belt {
    animation-play-state: paused;
  }

  .preset-chip,
  .stage-bg-glow {
    transition: none;
  }
}
</style>
