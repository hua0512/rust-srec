<script setup lang="ts">
import { computed, ref } from 'vue'
import { useData, withBase } from 'vitepress'

const { lang } = useData()
const isZh = computed(() => lang.value.startsWith('zh'))

const copiedQq = ref(false)
const qqGroupNumber = '566418735'

async function copyQqNumber() {
  try {
    if (typeof navigator !== 'undefined' && navigator.clipboard) {
      await navigator.clipboard.writeText(qqGroupNumber)
      copiedQq.value = true
      setTimeout(() => {
        copiedQq.value = false
      }, 2000)
    }
  } catch {
    copiedQq.value = false
  }
}

const supportCards = computed(() => [
  {
    id: 'star',
    title: isZh.value ? '在 GitHub 点亮 Star' : 'Star on GitHub',
    desc: isZh.value
      ? '最简单直接的支持方式，帮助更多需要自托管直播录制工具的人发现项目。'
      : 'The simplest way to support Rust-Srec and help more self-hosters discover the project.',
    cta: isZh.value ? '前往 GitHub 仓库' : 'Star repository',
    href: 'https://github.com/hua0512/rust-srec',
    external: true,
  },
  {
    id: 'sponsors',
    title: 'GitHub Sponsors',
    desc: isZh.value
      ? '通过 GitHub Sponsors 持续赞助项目维护、新平台适配与桌面端构建发布。'
      : 'Back ongoing maintenance, new platform extractors, and cross-platform builds via GitHub Sponsors.',
    cta: isZh.value ? '前往 GitHub Sponsors' : 'Sponsor on GitHub',
    href: 'https://github.com/sponsors/hua0512',
    external: true,
  },
  {
    id: 'contribute',
    title: isZh.value ? '参与贡献' : 'Contribute',
    desc: isZh.value
      ? '反馈 Bug、提交 Pull Request、改进中英文文档或补充新平台解析器。'
      : 'Report bugs, open pull requests, help with translations, or improve documentation.',
    cta: isZh.value ? '查看 Issues 与贡献' : 'Browse issues',
    href: 'https://github.com/hua0512/rust-srec/issues',
    external: true,
  },
  {
    id: 'premium',
    title: isZh.value ? '筹备中的高级版（Premium）' : 'Upcoming Premium Tier',
    desc: isZh.value
      ? '了解正在规划中的大规模批量监测扩展能力——开源核心功能将永久免费。'
      : 'See the planned high-scale batch monitoring extensions while core features stay free forever.',
    cta: isZh.value ? '查看版本对比' : 'View tier comparison',
    href: withBase(isZh.value ? '/zh/#tiers' : '/en/#tiers'),
    external: false,
  },
])
</script>

<template>
  <div class="support-root">
    <div class="support-grid">
      <a
        v-for="card in supportCards"
        :key="card.id"
        :href="card.href"
        :target="card.external ? '_blank' : undefined"
        :rel="card.external ? 'noopener noreferrer' : undefined"
        class="support-card"
      >
        <div class="support-card-head">
          <span class="support-icon" aria-hidden="true">
            <!-- Star -->
            <svg v-if="card.id === 'star'" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <polygon points="12 2 15.09 8.26 22 9.27 17 14.14 18.18 21.02 12 17.77 5.82 21.02 7 14.14 2 9.27 8.91 8.26 12 2" />
            </svg>
            <!-- Heart / Sponsor -->
            <svg v-else-if="card.id === 'sponsors'" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <path d="M20.84 4.61a5.5 5.5 0 0 0-7.78 0L12 5.67l-1.06-1.06a5.5 5.5 0 0 0-7.78 7.78l1.06 1.06L12 21.23l7.78-7.78 1.06-1.06a5.5 5.5 0 0 0 0-7.78z" />
            </svg>
            <!-- Contribute -->
            <svg v-else-if="card.id === 'contribute'" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <circle cx="18" cy="18" r="3" />
              <circle cx="6" cy="6" r="3" />
              <path d="M13 6h3a2 2 0 0 1 2 2v7" />
              <line x1="6" y1="9" x2="6" y2="21" />
            </svg>
            <!-- Upcoming Premium -->
            <svg v-else width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <path d="M12 2L2 7l10 5 10-5-10-5z" />
              <path d="M2 17l10 5 10-5" />
              <path d="M2 12l10 5 10-5" />
            </svg>
          </span>
          <span class="support-title">{{ card.title }}</span>
        </div>
        <p class="support-desc">{{ card.desc }}</p>
        <span class="support-cta">{{ card.cta }} →</span>
      </a>
    </div>

    <div v-if="isZh" class="qr-section">
      <div class="qr-grid">
        <div class="qr-card">
          <div class="qr-card-title">微信支付</div>
          <div class="qr-frame">
            <img :src="withBase('/images/donate_wx_qr.jpg')" alt="微信支付二维码" class="qr-img" loading="lazy" />
          </div>
          <p class="qr-note">微信扫码赞助项目持续开发</p>
        </div>

        <div class="qr-card">
          <div class="qr-card-title">支付宝</div>
          <div class="qr-frame">
            <img :src="withBase('/images/donate_alipay_qr.png')" alt="支付宝二维码" class="qr-img" loading="lazy" />
          </div>
          <p class="qr-note">支付宝扫码赞助项目持续开发</p>
        </div>

        <div class="qr-card">
          <div class="qr-card-title">QQ 交流群 · {{ qqGroupNumber }}</div>
          <div class="qr-frame">
            <img :src="withBase('/images/qq_group_zh.jpg')" alt="QQ 交流群二维码" class="qr-img" loading="lazy" />
          </div>
          <div class="qr-actions">
            <a
              class="qr-link-btn"
              href="https://qm.qq.com/q/qAbmjCuTug"
              target="_blank"
              rel="noopener noreferrer"
            >
              加入群聊
            </a>
            <button type="button" class="qr-copy-btn" @click="copyQqNumber">
              {{ copiedQq ? '已复制群号' : '复制群号' }}
            </button>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.support-root {
  display: flex;
  flex-direction: column;
  gap: 24px;
  margin: 20px 0;
}

.support-grid {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 14px;
}

.support-card {
  display: flex;
  flex-direction: column;
  gap: 10px;
  padding: 20px;
  border-radius: 14px;
  border: 1px solid var(--vp-c-divider);
  background: var(--vp-c-bg-soft);
  text-decoration: none !important;
  transition:
    border-color 0.2s ease,
    background-color 0.2s ease,
    transform 0.2s ease,
    box-shadow 0.2s ease;
}

.support-card:hover {
  border-color: var(--vp-c-brand-1);
  background: var(--vp-c-bg-elv);
  transform: translateY(-2px);
  box-shadow: 0 10px 26px -14px rgba(228, 77, 38, 0.24);
}

.support-card-head {
  display: flex;
  align-items: center;
  gap: 10px;
}

.support-icon {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 34px;
  height: 34px;
  border-radius: 9px;
  background: var(--vp-c-brand-soft);
  color: var(--vp-c-brand-1);
  flex-shrink: 0;
}

.support-title {
  font-size: 15px;
  font-weight: 600;
  color: var(--vp-c-text-1);
}

.support-desc {
  margin: 0 !important;
  font-size: 13.5px;
  line-height: 1.6;
  color: var(--vp-c-text-2);
  flex: 1;
}

.support-cta {
  font-size: 13px;
  font-weight: 600;
  color: var(--vp-c-brand-1);
}

.qr-grid {
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  gap: 16px;
}

.qr-card {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 12px;
  padding: 20px;
  border-radius: 14px;
  border: 1px solid var(--vp-c-divider);
  background: var(--vp-c-bg-soft);
  text-align: center;
}

.qr-card-title {
  font-size: 15px;
  font-weight: 600;
  color: var(--vp-c-text-1);
}

/* Crisp white backing box so QR codes scan cleanly in dark mode */
.qr-frame {
  background: #ffffff;
  padding: 10px;
  border-radius: 12px;
  box-shadow: 0 2px 10px rgba(0, 0, 0, 0.08);
  display: flex;
  align-items: center;
  justify-content: center;
}

.qr-img {
  width: 196px;
  height: 196px;
  object-fit: contain;
  border-radius: 6px;
}

.qr-note {
  margin: 0 !important;
  font-size: 12.5px;
  color: var(--vp-c-text-2);
}

.qr-actions {
  display: flex;
  flex-wrap: wrap;
  justify-content: center;
  gap: 8px;
}

.qr-link-btn,
.qr-copy-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  padding: 6px 12px;
  border-radius: 8px;
  font-size: 12.5px;
  font-weight: 600;
  cursor: pointer;
  text-decoration: none !important;
  transition: border-color 0.2s ease, background-color 0.2s ease, color 0.2s ease;
}

.qr-link-btn {
  background: var(--vp-c-brand-3);
  color: #ffffff !important;
  border: 1px solid transparent;
}

.qr-link-btn:hover {
  background: var(--vp-c-brand-2);
}

.qr-copy-btn {
  background: var(--vp-c-bg);
  color: var(--vp-c-text-1);
  border: 1px solid var(--vp-c-divider);
}

.qr-copy-btn:hover {
  border-color: var(--vp-c-brand-1);
  color: var(--vp-c-brand-1);
}

@media (max-width: 820px) {
  .qr-grid {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }
}

@media (max-width: 600px) {
  .support-grid,
  .qr-grid {
    grid-template-columns: 1fr;
  }
}

@media (prefers-reduced-motion: reduce) {
  .support-card {
    transition: none;
    transform: none;
  }
}
</style>
