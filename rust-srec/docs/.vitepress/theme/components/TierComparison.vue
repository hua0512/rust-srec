<script setup lang="ts">
import { computed } from 'vue'
import { useData, withBase } from 'vitepress'

const { lang } = useData()
const isZh = computed(() => lang.value.startsWith('zh'))

const communityFeatures = computed(() =>
  isZh.value
    ? [
        '自托管桌面应用（Windows / macOS / Linux）与 Docker / NAS 部署',
        '内置 14+ 主流直播平台开播监测与录制',
        'Mesio、FFmpeg、Streamlink 多引擎与实时流修复',
        '同步弹幕录制、内置弹幕回放播放器与场次统计',
        '可视化 DAG 后处理工作流（封装、转码、封面、自定义脚本）',
        '支持 rclone 云盘与百度网盘自动上传',
        'Telegram、Discord、Gotify、邮件与 Webhook 多渠道通知',
        '完整 REST API、API 密钥与 MCP 集成',
        '内置免费批量开播检测（Bilibili、Twitch、Bigo Live）',
      ]
    : [
        'Self-hosted Desktop app (Windows, macOS, Linux) and Docker / NAS',
        'All 14+ built-in live streaming platforms out of the box',
        'Mesio, FFmpeg & Streamlink engines with real-time stream repair',
        'Synced Danmu & live chat capture, overlay replay, and session stats',
        'Visual DAG post-processing workflows (remux, transcode, thumbnails, scripts)',
        'Automated cloud uploads via rclone and Baidu Netdisk',
        'Multi-channel notifications (Telegram, Discord, Gotify, Email, Webhook)',
        'Full REST API, scoped API keys, and MCP server integration',
        'Free built-in batch live detection for Bilibili, Twitch, and Bigo Live',
      ],
)

const premiumFeatures = computed(() =>
  isZh.value
    ? [
        '面向海量频道的高效批量监测能力',
        '更多平台专属增强与深度自动化特性',
        '更多未公开能力 · 敬请期待',
      ]
    : [
        'High-efficiency monitoring built for large channel fleets',
        'Exclusive platform enhancements & advanced automation',
        'More unannounced capabilities — stay tuned',
      ],
)

const getStartedLink = computed(() =>
  withBase(isZh.value ? '/zh/getting-started/' : '/en/getting-started/'),
)
</script>

<template>
  <div id="tiers" class="tier-comparison">
    <div class="tier-grid">
      <!-- Community Card -->
      <article class="tier-card tier-community">
        <div class="tier-header">
          <div class="tier-badges">
            <span class="tier-pill tier-pill-brand">
              {{ isZh ? '开源免费 · 永久使用' : 'Open Source · Free Forever' }}
            </span>
            <span class="tier-license">MIT / Apache-2.0</span>
          </div>
          <h3 class="tier-name">{{ isZh ? '社区版（Community）' : 'Community' }}</h3>
          <p class="tier-desc">
            {{
              isZh
                ? '完整可用的自托管直播录制与后处理套件，无功能锁、无强制订阅。'
                : 'The complete self-hosted recording, Danmu, and workflow suite with no feature locks.'
            }}
          </p>
        </div>

        <ul class="tier-list">
          <li v-for="item in communityFeatures" :key="item" class="tier-item">
            <span class="tier-icon-wrap tier-icon-check" aria-hidden="true">
              <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.75" stroke-linecap="round" stroke-linejoin="round">
                <polyline points="20 6 9 17 4 12" />
              </svg>
            </span>
            <span>{{ item }}</span>
          </li>
        </ul>

        <div class="tier-footer">
          <a class="tier-cta tier-cta-primary" :href="getStartedLink">
            <span>{{ isZh ? '立即开始免费使用' : 'Get Started Free' }}</span>
            <span class="tier-cta-arrow" aria-hidden="true">→</span>
          </a>
        </div>
      </article>

      <!-- Premium Card -->
      <article class="tier-card tier-premium">
        <div class="tier-header">
          <div class="tier-badges">
            <span class="tier-pill tier-pill-premium">
              <span class="tier-pulse-dot" aria-hidden="true" />
              {{ isZh ? '尚未开放 · 敬请期待' : 'Coming Soon · Not Yet Open' }}
            </span>
          </div>
          <h3 class="tier-name">{{ isZh ? '高级版（Premium）' : 'Premium' }}</h3>
          <p class="tier-desc">
            {{
              isZh
                ? '专为大规模监控与进阶自托管场景打造的神秘扩展，仍在秘密打磨中。'
                : 'An upcoming expansion for large-scale monitoring and power users, currently in the works.'
            }}
          </p>
        </div>

        <div class="tier-premium-block">
          <p class="tier-sublabel">
            {{ isZh ? '包含社区版全部功能，并带来：' : 'Everything in Community, plus:' }}
          </p>
          <ul class="tier-list">
            <li v-for="(item, idx) in premiumFeatures" :key="item" class="tier-item">
              <span class="tier-icon-wrap tier-icon-spark" aria-hidden="true">
                <!-- Lightning bolt for batch monitoring items -->
                <svg
                  v-if="idx < 2"
                  width="13"
                  height="13"
                  viewBox="0 0 24 24"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="2.3"
                  stroke-linecap="round"
                  stroke-linejoin="round"
                >
                  <polygon points="13 2 3 14 12 14 11 22 21 10 12 10 13 2" />
                </svg>
                <!-- Sparkle for unannounced capabilities -->
                <svg
                  v-else
                  width="13"
                  height="13"
                  viewBox="0 0 24 24"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="2.2"
                  stroke-linecap="round"
                  stroke-linejoin="round"
                >
                  <path d="m12 3-1.9 5.8a2 2 0 0 1-1.3 1.3L3 12l5.8 1.9a2 2 0 0 1 1.3 1.3L12 21l1.9-5.8a2 2 0 0 1 1.3-1.3L21 12l-5.8-1.9a2 2 0 0 1-1.3-1.3Z" />
                </svg>
              </span>
              <span>{{ item }}</span>
            </li>
          </ul>

          <div class="tier-sealed-teaser">
            <div class="tier-sealed-head">
              <svg class="tier-sealed-lock" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                <rect width="18" height="11" x="3" y="11" rx="2" ry="2" />
                <path d="M7 11V7a5 5 0 0 1 10 0v4" />
              </svg>
              <span>
                {{
                  isZh
                    ? '完整能力与开放计划将在正式发布前揭晓'
                    : 'Full capabilities & launch details to be revealed soon'
                }}
              </span>
            </div>
            <div class="tier-sealed-bars" aria-hidden="true">
              <span class="tier-sealed-bar bar-1" />
              <span class="tier-sealed-bar bar-2" />
              <span class="tier-sealed-bar bar-3" />
            </div>
          </div>
        </div>

        <div class="tier-promise">
          <svg class="tier-promise-icon" width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z" />
            <path d="m9 12 2 2 4-4" />
          </svg>
          <span>
            {{
              isZh
                ? '开源承诺：核心录制、流修复、弹幕、工作流与全部社区版功能将继续以 MIT / Apache-2.0 协议永久免费开源。'
                : 'Open-source guarantee: Core recording, stream repair, Danmu, workflows, and self-hosting stay free forever under MIT / Apache-2.0.'
            }}
          </span>
        </div>

        <div class="tier-footer">
          <a
            class="tier-cta tier-cta-muted"
            href="https://github.com/hua0512/rust-srec/releases"
            target="_blank"
            rel="noopener noreferrer"
          >
            <span>{{ isZh ? '尚未开放 · 关注 GitHub 动态' : 'Not Yet Open · Follow on GitHub' }}</span>
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
              <path d="M7 17 17 7" />
              <path d="M7 7h10v10" />
            </svg>
          </a>
        </div>
      </article>
    </div>
  </div>
</template>

<style scoped>
.tier-comparison {
  scroll-margin-top: 88px;
}

.tier-grid {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 20px;
  align-items: stretch;
}

.tier-card {
  position: relative;
  overflow: hidden;
  display: flex;
  flex-direction: column;
  justify-content: space-between;
  gap: 20px;
  padding: 28px;
  border-radius: 16px;
  border: 1px solid var(--vp-c-divider);
  background: var(--vp-c-bg-soft);
  transition:
    border-color 0.25s ease,
    box-shadow 0.25s ease,
    transform 0.25s ease;
}

/* Community card: crisp top orange hairline accent + subtle hover glow */
.tier-community {
  border-color: color-mix(in srgb, var(--vp-c-brand-1) 42%, var(--vp-c-divider));
  background: var(--vp-c-bg-elv);
  box-shadow: 0 14px 36px -22px rgba(228, 77, 38, 0.26);
}

.tier-community::before {
  content: '';
  position: absolute;
  top: 0;
  left: 0;
  right: 0;
  height: 3px;
  background: linear-gradient(90deg, #e44d26 0%, #f16529 55%, #ff8456 100%);
  pointer-events: none;
}

.tier-community:hover {
  border-color: var(--vp-c-brand-1);
  box-shadow:
    0 18px 42px -22px rgba(228, 77, 38, 0.36),
    0 0 0 1px color-mix(in srgb, var(--vp-c-brand-1) 24%, transparent);
}

/* Premium card: gradient-bordered frosted showcase card with radial glow */
.tier-premium {
  border-color: transparent;
  background:
    radial-gradient(280px 210px at 94% 4%, rgba(124, 58, 237, 0.12), transparent 70%),
    radial-gradient(220px 180px at 76% 0%, rgba(228, 77, 38, 0.09), transparent 72%),
    color-mix(in srgb, var(--vp-c-bg-soft) 92%, var(--vp-c-bg-elv));
  backdrop-filter: blur(12px);
  box-shadow:
    0 16px 40px -26px rgba(124, 58, 237, 0.24),
    0 12px 32px -26px rgba(228, 77, 38, 0.18);
}

.tier-premium::before {
  content: '';
  position: absolute;
  inset: 0;
  border-radius: inherit;
  padding: 1px;
  background: linear-gradient(
    135deg,
    rgba(228, 77, 38, 0.45) 0%,
    rgba(124, 58, 237, 0.48) 55%,
    rgba(139, 92, 246, 0.22) 100%
  );
  -webkit-mask:
    linear-gradient(#fff 0 0) content-box,
    linear-gradient(#fff 0 0);
  -webkit-mask-composite: xor;
  mask:
    linear-gradient(#fff 0 0) content-box,
    linear-gradient(#fff 0 0);
  mask-composite: exclude;
  pointer-events: none;
}

:global(.dark) .tier-premium {
  background:
    radial-gradient(300px 220px at 94% 4%, rgba(139, 92, 246, 0.2), transparent 70%),
    radial-gradient(240px 190px at 74% 0%, rgba(241, 101, 41, 0.14), transparent 72%),
    rgba(24, 24, 27, 0.88);
  box-shadow:
    0 20px 48px -24px rgba(0, 0, 0, 0.55),
    0 14px 36px -24px rgba(139, 92, 246, 0.26);
}

:global(.dark) .tier-premium::before {
  background: linear-gradient(
    135deg,
    rgba(255, 122, 77, 0.52) 0%,
    rgba(167, 139, 250, 0.52) 55%,
    rgba(167, 139, 250, 0.2) 100%
  );
}

.tier-premium:hover {
  box-shadow:
    0 20px 46px -24px rgba(124, 58, 237, 0.3),
    0 14px 36px -24px rgba(228, 77, 38, 0.24);
}

.tier-header {
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.tier-badges {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px;
}

.tier-pill {
  display: inline-flex;
  align-items: center;
  gap: 7px;
  padding: 3px 11px;
  border-radius: 999px;
  font-size: 12px;
  font-weight: 600;
}

.tier-pill-brand {
  background: var(--vp-c-brand-soft);
  color: var(--vp-c-brand-1);
}

.tier-pill-premium {
  background: linear-gradient(
    90deg,
    rgba(245, 158, 11, 0.14) 0%,
    rgba(124, 58, 237, 0.14) 100%
  );
  border: 1px solid rgba(124, 58, 237, 0.28);
  color: #6d28d9;
}

:global(.dark) .tier-pill-premium {
  background: linear-gradient(
    90deg,
    rgba(245, 158, 11, 0.16) 0%,
    rgba(167, 139, 250, 0.18) 100%
  );
  border-color: rgba(167, 139, 250, 0.34);
  color: #ddd6fe;
}

.tier-pulse-dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: linear-gradient(135deg, #f59e0b, #8b5cf6);
  box-shadow: 0 0 0 3px rgba(139, 92, 246, 0.22);
  animation: tier-dot-pulse 2.2s ease-in-out infinite;
}

@keyframes tier-dot-pulse {
  0%,
  100% {
    opacity: 1;
    transform: scale(1);
  }
  50% {
    opacity: 0.5;
    transform: scale(0.84);
  }
}

.tier-license {
  font-family: var(--vp-font-family-mono);
  font-size: 11px;
  color: var(--vp-c-text-3);
}

.tier-name {
  margin: 0 !important;
  padding: 0 !important;
  border: none !important;
  font-size: 22px !important;
  font-weight: 700;
  color: var(--vp-c-text-1);
}

.tier-desc {
  margin: 0 !important;
  font-size: 14px;
  line-height: 1.6;
  color: var(--vp-c-text-2);
}

.tier-premium-block {
  flex: 1;
  display: flex;
  flex-direction: column;
}

.tier-sublabel {
  margin: 0 0 12px !important;
  font-size: 13px;
  font-weight: 600;
  color: var(--vp-c-text-1);
}

.tier-list {
  list-style: none !important;
  margin: 0 !important;
  padding: 0 !important;
  display: flex;
  flex-direction: column;
  gap: 10px;
  flex: 1;
}

.tier-item {
  display: flex;
  align-items: flex-start;
  gap: 10px;
  font-size: 14px;
  line-height: 1.5;
  color: var(--vp-c-text-1);
  margin: 0 !important;
}

.tier-icon-wrap {
  flex-shrink: 0;
  width: 20px;
  height: 20px;
  margin-top: 1px;
  border-radius: 6px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
}

.tier-icon-check {
  background: var(--vp-c-brand-soft);
  color: var(--vp-c-brand-1);
}

.tier-icon-spark {
  background: linear-gradient(135deg, rgba(228, 77, 38, 0.14), rgba(124, 58, 237, 0.16));
  color: #7c3aed;
}

:global(.dark) .tier-icon-spark {
  background: linear-gradient(135deg, rgba(241, 101, 41, 0.18), rgba(167, 139, 250, 0.22));
  color: #c4b5fd;
}

.tier-promise {
  display: flex;
  align-items: flex-start;
  gap: 9px;
  padding: 12px 14px;
  border-radius: 10px;
  border: 1px solid var(--vp-c-divider);
  background: color-mix(in srgb, var(--vp-c-bg) 82%, transparent);
  font-size: 13px;
  line-height: 1.55;
  color: var(--vp-c-text-2);
}

.tier-sealed-teaser {
  margin-top: 16px;
  padding: 12px 14px;
  border-radius: 12px;
  border: 1px dashed rgba(124, 58, 237, 0.28);
  background: linear-gradient(
    135deg,
    rgba(228, 77, 38, 0.05) 0%,
    rgba(124, 58, 237, 0.08) 100%
  );
  display: flex;
  flex-direction: column;
  gap: 10px;
}

:global(.dark) .tier-sealed-teaser {
  border-color: rgba(167, 139, 250, 0.3);
  background: linear-gradient(
    135deg,
    rgba(241, 101, 41, 0.08) 0%,
    rgba(167, 139, 250, 0.12) 100%
  );
}

.tier-sealed-head {
  display: flex;
  align-items: center;
  gap: 7px;
  font-size: 12px;
  font-weight: 600;
  color: var(--vp-c-text-2);
}

.tier-sealed-lock {
  flex-shrink: 0;
  color: #7c3aed;
}

:global(.dark) .tier-sealed-lock {
  color: #c4b5fd;
}

.tier-sealed-bars {
  display: flex;
  flex-direction: column;
  gap: 6px;
  filter: blur(0.4px);
}

.tier-sealed-bar {
  display: block;
  height: 7px;
  border-radius: 999px;
  background: linear-gradient(
    90deg,
    rgba(124, 58, 237, 0.14) 0%,
    rgba(228, 77, 38, 0.22) 50%,
    rgba(124, 58, 237, 0.14) 100%
  );
  background-size: 200% 100%;
  animation: tier-sealed-shimmer 3.2s ease-in-out infinite;
}

:global(.dark) .tier-sealed-bar {
  background: linear-gradient(
    90deg,
    rgba(167, 139, 250, 0.16) 0%,
    rgba(255, 122, 77, 0.24) 50%,
    rgba(167, 139, 250, 0.16) 100%
  );
  background-size: 200% 100%;
}

.tier-sealed-bar.bar-1 {
  width: 82%;
}

.tier-sealed-bar.bar-2 {
  width: 64%;
  animation-delay: 0.4s;
}

.tier-sealed-bar.bar-3 {
  width: 72%;
  animation-delay: 0.8s;
}

@keyframes tier-sealed-shimmer {
  0%,
  100% {
    background-position: 0% 50%;
    opacity: 0.72;
  }
  50% {
    background-position: 100% 50%;
    opacity: 1;
  }
}

.tier-promise-icon {
  flex-shrink: 0;
  margin-top: 2px;
  color: var(--vp-c-brand-1);
}

.tier-footer {
  padding-top: 4px;
}

.tier-cta {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 7px;
  width: 100%;
  padding: 11px 18px;
  border-radius: 10px;
  font-size: 14px;
  font-weight: 600;
  text-decoration: none !important;
  transition:
    background-color 0.2s ease,
    border-color 0.2s ease,
    color 0.2s ease,
    box-shadow 0.2s ease;
}

.tier-cta-primary {
  background: var(--vp-c-brand-3);
  color: #ffffff !important;
  border: 1px solid transparent;
}

.tier-cta-primary:hover {
  background: var(--vp-c-brand-2);
  box-shadow: 0 8px 20px -10px rgba(228, 77, 38, 0.55);
}

.tier-cta-arrow {
  transition: transform 0.2s ease;
}

.tier-cta-primary:hover .tier-cta-arrow {
  transform: translateX(2px);
}

.tier-cta-muted {
  background: color-mix(in srgb, var(--vp-c-bg) 86%, transparent);
  color: var(--vp-c-text-2) !important;
  border: 1px solid var(--vp-c-divider);
}

.tier-cta-muted:hover {
  border-color: color-mix(in srgb, #8b5cf6 48%, var(--vp-c-divider));
  color: var(--vp-c-text-1) !important;
  background: var(--vp-c-bg);
}

@media (max-width: 768px) {
  .tier-grid {
    grid-template-columns: 1fr;
  }

  .tier-card {
    padding: 22px;
  }
}

@media (prefers-reduced-motion: reduce) {
  .tier-pulse-dot,
  .tier-sealed-bar {
    animation: none;
  }

  .tier-card,
  .tier-cta,
  .tier-cta-arrow {
    transition: none;
    transform: none;
  }
}
</style>
