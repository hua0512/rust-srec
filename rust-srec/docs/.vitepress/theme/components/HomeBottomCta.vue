<script setup lang="ts">
import { computed } from 'vue'
import { useData, withBase } from 'vitepress'

const { lang } = useData()
const isZh = computed(() => lang.value.startsWith('zh'))

const getStartedHref = computed(() =>
  withBase(isZh.value ? '/zh/getting-started/' : '/en/getting-started/'),
)
const dockerHref = computed(() =>
  withBase(isZh.value ? '/zh/getting-started/docker' : '/en/getting-started/docker'),
)
const releaseNotesHref = computed(() =>
  withBase(isZh.value ? '/zh/release-notes/' : '/en/release-notes/'),
)
</script>

<template>
  <section class="bottom-cta">
    <div class="bottom-cta-glow" aria-hidden="true" />
    <div class="bottom-cta-dots" aria-hidden="true" />

    <div class="bottom-cta-inner">
      <span class="bottom-cta-eyebrow">
        {{ isZh ? '开始使用' : 'GET STARTED' }}
      </span>
      <h2 class="bottom-cta-title">
        {{
          isZh
            ? '准备好全自动留住每一场精彩直播了吗？'
            : 'Ready to capture every broadcast automatically?'
        }}
      </h2>
      <p class="bottom-cta-subtitle">
        {{
          isZh
            ? '几分钟即可在你的电脑、家用服务器或 NAS 上运行——完全开源自托管，全天候稳定录制视频与弹幕。'
            : 'Deploy in minutes on your desktop, home server, or NAS — open source, self-hosted, and built for nonstop recording.'
        }}
      </p>

      <div class="bottom-cta-actions">
        <a class="cta-btn cta-btn-brand" :href="getStartedHref">
          <span>{{ isZh ? '快速开始' : 'Get Started' }}</span>
          <span class="cta-arrow" aria-hidden="true">→</span>
        </a>
        <a class="cta-btn cta-btn-secondary" :href="dockerHref">
          <span>{{ isZh ? 'Docker 部署' : 'Docker Setup' }}</span>
        </a>
        <a class="cta-btn cta-btn-ghost" :href="releaseNotesHref">
          <span>{{ isZh ? '更新日志' : 'Release Notes' }}</span>
        </a>
      </div>
    </div>
  </section>
</template>

<style scoped>
.bottom-cta {
  position: relative;
  overflow: hidden;
  border-radius: 20px;
  border: 1px solid var(--vp-c-divider);
  background: var(--vp-c-bg-alt);
  padding: 56px 28px;
  text-align: center;
  box-shadow:
    inset 0 1px 0 rgba(255, 255, 255, 0.55),
    0 18px 42px -28px rgba(0, 0, 0, 0.16);
}

:global(.dark) .bottom-cta {
  background: linear-gradient(180deg, #141418 0%, #0f0f12 100%);
  border-color: #27272a;
  box-shadow:
    inset 0 1px 0 rgba(255, 255, 255, 0.06),
    0 24px 60px -28px rgba(0, 0, 0, 0.72);
}

.bottom-cta-glow {
  position: absolute;
  inset: 0;
  background:
    radial-gradient(52% 90% at 18% 0%, rgba(228, 77, 38, 0.13), transparent 70%),
    radial-gradient(48% 85% at 84% 100%, rgba(124, 58, 237, 0.11), transparent 72%);
  pointer-events: none;
}

:global(.dark) .bottom-cta-glow {
  background:
    radial-gradient(52% 90% at 18% 0%, rgba(241, 101, 41, 0.2), transparent 70%),
    radial-gradient(48% 85% at 84% 100%, rgba(139, 92, 246, 0.18), transparent 72%);
}

.bottom-cta-dots {
  position: absolute;
  inset: 0;
  background-image: radial-gradient(rgba(24, 24, 27, 0.08) 1px, transparent 1px);
  background-size: 20px 20px;
  mask-image: radial-gradient(ellipse 75% 75% at 50% 50%, #000 20%, transparent 80%);
  -webkit-mask-image: radial-gradient(ellipse 75% 75% at 50% 50%, #000 20%, transparent 80%);
  pointer-events: none;
}

:global(.dark) .bottom-cta-dots {
  background-image: radial-gradient(rgba(255, 255, 255, 0.08) 1px, transparent 1px);
}

.bottom-cta-inner {
  position: relative;
  z-index: 1;
  max-width: 680px;
  margin: 0 auto;
}

.bottom-cta-eyebrow {
  display: inline-flex;
  align-items: center;
  padding: 4px 12px;
  margin-bottom: 16px;
  border-radius: 9999px;
  background: var(--vp-c-brand-soft);
  border: 1px solid rgba(228, 77, 38, 0.24);
  color: var(--vp-c-brand-1);
  font-family: var(--vp-font-family-mono);
  font-size: 11.5px;
  font-weight: 600;
  letter-spacing: 0.08em;
  text-transform: uppercase;
}

.bottom-cta-title {
  margin: 0 !important;
  padding: 0 !important;
  border: none !important;
  font-size: clamp(1.625rem, 3vw, 2.25rem) !important;
  font-weight: 700 !important;
  line-height: 1.2 !important;
  letter-spacing: -0.025em !important;
  color: var(--vp-c-text-1);
}

.bottom-cta-subtitle {
  margin: 14px auto 0;
  max-width: 560px;
  font-size: 15.5px;
  line-height: 1.65;
  color: var(--vp-c-text-2);
}

.bottom-cta-actions {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  justify-content: center;
  gap: 12px;
  margin-top: 28px;
}

.cta-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 7px;
  padding: 10px 20px;
  border-radius: 9999px;
  font-size: 14px;
  font-weight: 600;
  line-height: 1.3;
  text-decoration: none !important;
  transition:
    transform 0.18s ease,
    border-color 0.18s ease,
    background-color 0.18s ease,
    box-shadow 0.18s ease,
    color 0.18s ease;
}

.cta-btn:focus-visible {
  outline: 2px solid var(--vp-c-brand-1);
  outline-offset: 2px;
}

.cta-btn-brand {
  color: #ffffff !important;
  border: 1px solid rgba(255, 255, 255, 0.16);
  background: linear-gradient(135deg, #e44d26 0%, #c23b16 100%);
  box-shadow:
    inset 0 1px 0 rgba(255, 255, 255, 0.25),
    0 8px 22px -8px rgba(228, 77, 38, 0.55);
}

:global(.dark) .cta-btn-brand {
  background: linear-gradient(135deg, #ff7a4d 0%, #e44d26 100%);
  color: #ffffff !important;
}

.cta-btn-brand:hover {
  transform: translateY(-1px);
  box-shadow:
    inset 0 1px 0 rgba(255, 255, 255, 0.3),
    0 12px 28px -8px rgba(228, 77, 38, 0.65);
}

.cta-arrow {
  transition: transform 0.18s ease;
}

.cta-btn-brand:hover .cta-arrow {
  transform: translateX(2px);
}

.cta-btn-secondary {
  color: var(--vp-c-text-1) !important;
  border: 1px solid var(--vp-c-border);
  background: var(--vp-c-bg-elv);
  box-shadow: 0 2px 8px -4px rgba(0, 0, 0, 0.08);
}

:global(.dark) .cta-btn-secondary {
  background: #1f1f23;
  border-color: #3f3f46;
}

.cta-btn-secondary:hover {
  transform: translateY(-1px);
  border-color: var(--vp-c-brand-1);
  color: var(--vp-c-brand-1) !important;
}

.cta-btn-ghost {
  color: var(--vp-c-text-2) !important;
  border: 1px solid transparent;
  background: transparent;
}

.cta-btn-ghost:hover {
  color: var(--vp-c-text-1) !important;
  background: var(--vp-c-bg-soft);
  border-color: var(--vp-c-divider);
}

@media (min-width: 640px) {
  .bottom-cta {
    padding: 68px 48px;
  }
}

@media (prefers-reduced-motion: reduce) {
  .cta-btn,
  .cta-arrow {
    transition: none;
  }

  .cta-btn:hover,
  .cta-btn-brand:hover .cta-arrow {
    transform: none;
  }
}
</style>
