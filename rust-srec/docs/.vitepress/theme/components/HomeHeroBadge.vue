<script setup lang="ts">
import { computed } from 'vue'
import { useData, withBase } from 'vitepress'

const { lang } = useData()
const isZh = computed(() => lang.value.startsWith('zh'))

const releaseHref = computed(() =>
  withBase(isZh.value ? '/zh/release-notes/v0.8.0' : '/en/release-notes/v0.8.0'),
)
</script>

<template>
  <div class="hero-badge-wrap">
    <a class="hero-badge" :href="releaseHref">
      <span class="hero-badge-dot-wrap" aria-hidden="true">
        <span class="hero-badge-ping" />
        <span class="hero-badge-dot" />
      </span>
      <span class="hero-badge-version">v0.8.0</span>
      <span class="hero-badge-sep" aria-hidden="true" />
      <span class="hero-badge-text">
        {{
          isZh
            ? '开源自托管 · 桌面端 & Docker · 14+ 平台'
            : 'Open Source · Desktop & Docker · 14+ Platforms'
        }}
      </span>
      <span class="hero-badge-arrow" aria-hidden="true">→</span>
    </a>
  </div>
</template>

<style scoped>
.hero-badge-wrap {
  display: flex;
  justify-content: center;
  margin-bottom: 16px;
}

@media (min-width: 60rem) {
  .hero-badge-wrap {
    justify-content: flex-start;
  }
}

.hero-badge {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  max-width: 100%;
  padding: 5px 12px 5px 9px;
  border-radius: 9999px;
  border: 1px solid var(--vp-c-divider);
  background: color-mix(in srgb, var(--vp-c-bg-elv) 80%, transparent);
  backdrop-filter: blur(10px);
  -webkit-backdrop-filter: blur(10px);
  color: var(--vp-c-text-2);
  font-size: 12.5px;
  line-height: 1.3;
  text-decoration: none;
  box-shadow:
    inset 0 1px 0 rgba(255, 255, 255, 0.45),
    0 2px 10px -4px rgba(0, 0, 0, 0.08);
  transition:
    border-color 0.2s ease,
    background-color 0.2s ease,
    color 0.2s ease,
    box-shadow 0.2s ease,
    transform 0.2s ease;
}

:global(.dark) .hero-badge {
  background: rgba(24, 24, 27, 0.72);
  box-shadow:
    inset 0 1px 0 rgba(255, 255, 255, 0.06),
    0 4px 16px -6px rgba(0, 0, 0, 0.45);
}

.hero-badge:hover {
  border-color: var(--vp-c-brand-1);
  color: var(--vp-c-text-1);
  box-shadow:
    inset 0 1px 0 rgba(255, 255, 255, 0.14),
    0 6px 20px -8px rgba(228, 77, 38, 0.35);
  transform: translateY(-1px);
}

.hero-badge:focus-visible {
  outline: 2px solid var(--vp-c-brand-1);
  outline-offset: 2px;
}

.hero-badge-dot-wrap {
  position: relative;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 8px;
  height: 8px;
  flex-shrink: 0;
}

.hero-badge-ping {
  position: absolute;
  inset: -2px;
  border-radius: 9999px;
  background: #f16529;
  opacity: 0.55;
  animation: hero-badge-pulse 2.2s cubic-bezier(0, 0, 0.2, 1) infinite;
}

.hero-badge-dot {
  position: relative;
  width: 7px;
  height: 7px;
  border-radius: 9999px;
  background: #f16529;
  box-shadow: 0 0 8px rgba(241, 101, 41, 0.65);
}

.hero-badge-version {
  font-family: var(--vp-font-family-mono);
  font-size: 11.5px;
  font-weight: 600;
  letter-spacing: -0.01em;
  color: var(--vp-c-brand-1);
  background: var(--vp-c-brand-soft);
  padding: 1px 7px;
  border-radius: 9999px;
  flex-shrink: 0;
}

.hero-badge-sep {
  width: 1px;
  height: 12px;
  background: var(--vp-c-divider);
  flex-shrink: 0;
}

.hero-badge-text {
  font-weight: 500;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.hero-badge-arrow {
  color: var(--vp-c-text-3);
  font-weight: 600;
  flex-shrink: 0;
  transition:
    transform 0.2s ease,
    color 0.2s ease;
}

.hero-badge:hover .hero-badge-arrow {
  color: var(--vp-c-brand-1);
  transform: translateX(2px);
}

@keyframes hero-badge-pulse {
  0% {
    transform: scale(0.85);
    opacity: 0.65;
  }
  70%,
  100% {
    transform: scale(2.1);
    opacity: 0;
  }
}

@media (max-width: 480px) {
  .hero-badge {
    font-size: 11.5px;
    gap: 6px;
    padding: 4px 10px 4px 8px;
  }

  .hero-badge-sep {
    display: none;
  }
}

@media (prefers-reduced-motion: reduce) {
  .hero-badge-ping {
    animation: none;
    opacity: 0;
  }

  .hero-badge,
  .hero-badge-arrow {
    transition: none;
  }

  .hero-badge:hover,
  .hero-badge:hover .hero-badge-arrow {
    transform: none;
  }
}
</style>
