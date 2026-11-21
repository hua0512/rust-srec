<script setup lang="ts">
import { computed, ref } from 'vue'
import { useData, withBase } from 'vitepress'

const { lang } = useData()
const isZh = computed(() => lang.value.startsWith('zh'))

interface PlatformEntry {
  id: string
  nameEn: string
  nameZh: string
  protocol: string
  danmu: boolean
  color: string
  pathEn: string
  pathZh: string
}

// Exact 14 built-in platforms; exactly 9 have built-in Danmu providers
// (Bilibili, Douyin, Douyu, Huya, Twitch, SOOP, Bigo Live, TikTok, Twitcasting).
const platforms: PlatformEntry[] = [
  {
    id: 'bilibili',
    nameEn: 'Bilibili',
    nameZh: 'Bilibili',
    protocol: 'FLV / HLS',
    danmu: true,
    color: '#fb7299',
    pathEn: '/en/platforms/bilibili',
    pathZh: '/zh/platforms/bilibili',
  },
  {
    id: 'douyin',
    nameEn: 'Douyin',
    nameZh: '抖音',
    protocol: 'FLV / HLS',
    danmu: true,
    color: '#fe2c55',
    pathEn: '/en/platforms/douyin',
    pathZh: '/zh/platforms/douyin',
  },
  {
    id: 'douyu',
    nameEn: 'Douyu',
    nameZh: '斗鱼',
    protocol: 'FLV',
    danmu: true,
    color: '#ff6a00',
    pathEn: '/en/platforms/douyu',
    pathZh: '/zh/platforms/douyu',
  },
  {
    id: 'huya',
    nameEn: 'Huya',
    nameZh: '虎牙',
    protocol: 'FLV / HLS',
    danmu: true,
    color: '#f59e0b',
    pathEn: '/en/platforms/huya',
    pathZh: '/zh/platforms/huya',
  },
  {
    id: 'twitch',
    nameEn: 'Twitch',
    nameZh: 'Twitch',
    protocol: 'HLS',
    danmu: true,
    color: '#9146ff',
    pathEn: '/en/platforms/twitch',
    pathZh: '/zh/platforms/twitch',
  },
  {
    id: 'soop',
    nameEn: 'SOOP',
    nameZh: 'SOOP',
    protocol: 'HLS',
    danmu: true,
    color: '#3b82f6',
    pathEn: '/en/platforms/soop',
    pathZh: '/zh/platforms/soop',
  },
  {
    id: 'bigo',
    nameEn: 'Bigo Live',
    nameZh: 'Bigo Live',
    protocol: 'HLS',
    danmu: true,
    color: '#06b6d4',
    pathEn: '/en/platforms/bigo',
    pathZh: '/zh/platforms/bigo',
  },
  {
    id: 'tiktok',
    nameEn: 'TikTok',
    nameZh: 'TikTok',
    protocol: 'FLV / HLS',
    danmu: true,
    color: '#14b8a6',
    pathEn: '/en/platforms/tiktok',
    pathZh: '/zh/platforms/tiktok',
  },
  {
    id: 'twitcasting',
    nameEn: 'Twitcasting',
    nameZh: 'Twitcasting',
    protocol: 'HLS',
    danmu: true,
    color: '#2563eb',
    pathEn: '/en/platforms/others#twitcasting',
    pathZh: '/zh/platforms/others#twitcasting',
  },
  {
    id: 'pandatv',
    nameEn: 'PandaTV',
    nameZh: 'PandaTV',
    protocol: 'HLS',
    danmu: false,
    color: '#ec4899',
    pathEn: '/en/platforms/others#pandatv',
    pathZh: '/zh/platforms/others#pandatv',
  },
  {
    id: 'picarto',
    nameEn: 'Picarto',
    nameZh: 'Picarto',
    protocol: 'HLS / MP4',
    danmu: false,
    color: '#22c55e',
    pathEn: '/en/platforms/others#picarto',
    pathZh: '/zh/platforms/others#picarto',
  },
  {
    id: 'redbook',
    nameEn: 'Redbook',
    nameZh: '小红书',
    protocol: 'FLV / HLS',
    danmu: false,
    color: '#ff2442',
    pathEn: '/en/platforms/others#redbook-小红书',
    pathZh: '/zh/platforms/others#小红书',
  },
  {
    id: 'weibo',
    nameEn: 'Weibo',
    nameZh: '微博',
    protocol: 'FLV / HLS',
    danmu: false,
    color: '#eab308',
    pathEn: '/en/platforms/others#weibo',
    pathZh: '/zh/platforms/others#微博',
  },
  {
    id: 'acfun',
    nameEn: 'AcFun',
    nameZh: 'AcFun',
    protocol: 'FLV',
    danmu: false,
    color: '#fd4c5d',
    pathEn: '/en/platforms/others#acfun',
    pathZh: '/zh/platforms/others#acfun',
  },
]

const rowOne = platforms.slice(0, 7)
const rowTwo = platforms.slice(7)

const viewMode = ref<'marquee' | 'all' | 'danmu'>('marquee')

const visiblePlatforms = computed(() =>
  viewMode.value === 'danmu' ? platforms.filter((p) => p.danmu) : platforms,
)

const overviewLink = computed(() =>
  withBase(isZh.value ? '/zh/platforms/' : '/en/platforms/'),
)
</script>

<template>
  <div class="platform-marquee">
    <div class="platform-toolbar">
      <div class="platform-filters" role="group" :aria-label="isZh ? '筛选与切换视图' : 'Filter and switch platform view'">
        <button
          type="button"
          class="filter-pill"
          :class="{ 'is-active': viewMode === 'marquee' }"
          :aria-pressed="viewMode === 'marquee'"
          @click="viewMode = 'marquee'"
        >
          {{ isZh ? '跑马灯' : 'Showcase' }}
        </button>
        <button
          type="button"
          class="filter-pill"
          :class="{ 'is-active': viewMode === 'all' }"
          :aria-pressed="viewMode === 'all'"
          @click="viewMode = 'all'"
        >
          {{ isZh ? '全部网格 (14)' : 'All Grid (14)' }}
        </button>
        <button
          type="button"
          class="filter-pill"
          :class="{ 'is-active': viewMode === 'danmu' }"
          :aria-pressed="viewMode === 'danmu'"
          @click="viewMode = 'danmu'"
        >
          {{ isZh ? '支持弹幕 (9)' : 'With Danmu (9)' }}
        </button>
      </div>
      <a class="platform-overview-link" :href="overviewLink">
        <span>{{ isZh ? '查看所有平台配置与能力矩阵' : 'View full platform guide & matrix' }}</span>
        <span aria-hidden="true">→</span>
      </a>
    </div>

    <!-- Dual-row infinite marquee -->
    <div v-if="viewMode === 'marquee'" class="marquee-stack">
      <div class="marquee-row">
        <div class="marquee-track marquee-track-left">
          <div class="marquee-group">
            <a
              v-for="item in rowOne"
              :key="item.id"
              :href="withBase(isZh ? item.pathZh : item.pathEn)"
              class="platform-card platform-card-marquee"
              :style="{ '--platform-accent': item.color }"
            >
              <div class="platform-card-top">
                <span class="platform-name-wrap">
                  <span class="platform-dot" aria-hidden="true" />
                  <span class="platform-name">{{ isZh ? item.nameZh : item.nameEn }}</span>
                </span>
                <span v-if="item.danmu" class="danmu-badge">
                  {{ isZh ? '弹幕' : 'Danmu' }}
                </span>
              </div>
              <div class="platform-card-bottom">
                <span class="platform-proto">{{ item.protocol }}</span>
                <span class="platform-arrow" aria-hidden="true">→</span>
              </div>
            </a>
          </div>

          <div class="marquee-group marquee-group-dup" aria-hidden="true">
            <a
              v-for="item in rowOne"
              :key="`dup-1-${item.id}`"
              :href="withBase(isZh ? item.pathZh : item.pathEn)"
              tabindex="-1"
              class="platform-card platform-card-marquee"
              :style="{ '--platform-accent': item.color }"
            >
              <div class="platform-card-top">
                <span class="platform-name-wrap">
                  <span class="platform-dot" aria-hidden="true" />
                  <span class="platform-name">{{ isZh ? item.nameZh : item.nameEn }}</span>
                </span>
                <span v-if="item.danmu" class="danmu-badge">
                  {{ isZh ? '弹幕' : 'Danmu' }}
                </span>
              </div>
              <div class="platform-card-bottom">
                <span class="platform-proto">{{ item.protocol }}</span>
                <span class="platform-arrow" aria-hidden="true">→</span>
              </div>
            </a>
          </div>
        </div>
      </div>

      <div class="marquee-row">
        <div class="marquee-track marquee-track-right">
          <div class="marquee-group">
            <a
              v-for="item in rowTwo"
              :key="item.id"
              :href="withBase(isZh ? item.pathZh : item.pathEn)"
              class="platform-card platform-card-marquee"
              :style="{ '--platform-accent': item.color }"
            >
              <div class="platform-card-top">
                <span class="platform-name-wrap">
                  <span class="platform-dot" aria-hidden="true" />
                  <span class="platform-name">{{ isZh ? item.nameZh : item.nameEn }}</span>
                </span>
                <span v-if="item.danmu" class="danmu-badge">
                  {{ isZh ? '弹幕' : 'Danmu' }}
                </span>
              </div>
              <div class="platform-card-bottom">
                <span class="platform-proto">{{ item.protocol }}</span>
                <span class="platform-arrow" aria-hidden="true">→</span>
              </div>
            </a>
          </div>

          <div class="marquee-group marquee-group-dup" aria-hidden="true">
            <a
              v-for="item in rowTwo"
              :key="`dup-2-${item.id}`"
              :href="withBase(isZh ? item.pathZh : item.pathEn)"
              tabindex="-1"
              class="platform-card platform-card-marquee"
              :style="{ '--platform-accent': item.color }"
            >
              <div class="platform-card-top">
                <span class="platform-name-wrap">
                  <span class="platform-dot" aria-hidden="true" />
                  <span class="platform-name">{{ isZh ? item.nameZh : item.nameEn }}</span>
                </span>
                <span v-if="item.danmu" class="danmu-badge">
                  {{ isZh ? '弹幕' : 'Danmu' }}
                </span>
              </div>
              <div class="platform-card-bottom">
                <span class="platform-proto">{{ item.protocol }}</span>
                <span class="platform-arrow" aria-hidden="true">→</span>
              </div>
            </a>
          </div>
        </div>
      </div>
    </div>

    <!-- Static grid mode (All 14 / With Danmu 9) -->
    <div v-else class="platform-grid">
      <a
        v-for="item in visiblePlatforms"
        :key="item.id"
        :href="withBase(isZh ? item.pathZh : item.pathEn)"
        class="platform-card"
        :style="{ '--platform-accent': item.color }"
      >
        <div class="platform-card-top">
          <span class="platform-name-wrap">
            <span class="platform-dot" aria-hidden="true" />
            <span class="platform-name">{{ isZh ? item.nameZh : item.nameEn }}</span>
          </span>
          <span v-if="item.danmu" class="danmu-badge">
            {{ isZh ? '弹幕' : 'Danmu' }}
          </span>
        </div>
        <div class="platform-card-bottom">
          <span class="platform-proto">{{ item.protocol }}</span>
          <span class="platform-arrow" aria-hidden="true">→</span>
        </div>
      </a>
    </div>
  </div>
</template>

<style scoped>
.platform-marquee {
  display: flex;
  flex-direction: column;
  gap: 18px;
}

.platform-toolbar {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
}

.platform-filters {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
}

.filter-pill {
  padding: 6px 14px;
  border-radius: 999px;
  border: 1px solid var(--vp-c-divider);
  background: var(--vp-c-bg-soft);
  color: var(--vp-c-text-2);
  font-size: 13px;
  font-weight: 500;
  cursor: pointer;
  transition: border-color 0.2s ease, background-color 0.2s ease, color 0.2s ease;
}

.filter-pill:hover {
  border-color: var(--vp-c-brand-1);
  color: var(--vp-c-text-1);
}

.filter-pill.is-active {
  border-color: var(--vp-c-brand-1);
  background: var(--vp-c-brand-soft);
  color: var(--vp-c-brand-1);
  font-weight: 600;
}

.platform-overview-link {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 13px;
  font-weight: 600;
  color: var(--vp-c-brand-1);
  text-decoration: none;
}

.platform-overview-link:hover {
  text-decoration: underline;
}

/* Dual-row infinite marquee */
.marquee-stack {
  display: flex;
  flex-direction: column;
  gap: 12px;
  -webkit-mask-image: linear-gradient(
    to right,
    transparent 0%,
    #000 7%,
    #000 93%,
    transparent 100%
  );
  mask-image: linear-gradient(
    to right,
    transparent 0%,
    #000 7%,
    #000 93%,
    transparent 100%
  );
}

.marquee-row {
  overflow: hidden;
  padding: 4px 0;
}

.marquee-track {
  display: flex;
  width: max-content;
  will-change: transform;
}

.marquee-track-left {
  animation: marquee-scroll-left 36s linear infinite;
}

.marquee-track-right {
  animation: marquee-scroll-right 40s linear infinite;
}

.marquee-row:hover .marquee-track,
.marquee-row:focus-within .marquee-track {
  animation-play-state: paused;
}

.marquee-group {
  display: flex;
  gap: 12px;
  padding-right: 12px;
  flex-shrink: 0;
}

@keyframes marquee-scroll-left {
  0% {
    transform: translateX(0);
  }
  100% {
    transform: translateX(-50%);
  }
}

@keyframes marquee-scroll-right {
  0% {
    transform: translateX(-50%);
  }
  100% {
    transform: translateX(0);
  }
}

/* Grid layout */
.platform-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(176px, 1fr));
  gap: 12px;
}

.platform-card {
  display: flex;
  flex-direction: column;
  justify-content: space-between;
  gap: 12px;
  padding: 14px 16px;
  border-radius: 12px;
  border: 1px solid var(--vp-c-divider);
  background: var(--vp-c-bg-soft);
  text-decoration: none !important;
  transition:
    border-color 0.2s ease,
    transform 0.2s ease,
    background-color 0.2s ease,
    box-shadow 0.2s ease;
}

.platform-card-marquee {
  width: 204px;
  flex-shrink: 0;
}

.platform-card:hover {
  border-color: color-mix(in srgb, var(--platform-accent) 68%, var(--vp-c-brand-1));
  background: var(--vp-c-bg-elv);
  transform: translateY(-2px);
  box-shadow: 0 10px 24px -14px color-mix(in srgb, var(--platform-accent) 46%, transparent);
}

.platform-card-top {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
}

.platform-name-wrap {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
}

.platform-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  flex-shrink: 0;
  background: var(--platform-accent);
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--platform-accent) 20%, transparent);
  transition: box-shadow 0.2s ease, transform 0.2s ease;
}

.platform-card:hover .platform-dot {
  transform: scale(1.12);
  box-shadow: 0 0 0 4px color-mix(in srgb, var(--platform-accent) 30%, transparent);
}

.platform-name {
  font-size: 15px;
  font-weight: 600;
  color: var(--vp-c-text-1);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.danmu-badge {
  display: inline-flex;
  align-items: center;
  padding: 2px 7px;
  border-radius: 999px;
  background: var(--vp-c-brand-soft);
  color: var(--vp-c-brand-1);
  font-size: 11px;
  font-weight: 600;
  line-height: 1.4;
  white-space: nowrap;
}

.platform-card-bottom {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
}

.platform-proto {
  font-family: var(--vp-font-family-mono);
  font-size: 11px;
  color: var(--vp-c-text-3);
}

.platform-arrow {
  font-size: 13px;
  color: var(--vp-c-text-3);
  transition: transform 0.2s ease, color 0.2s ease;
}

.platform-card:hover .platform-arrow {
  color: var(--platform-accent);
  transform: translateX(2px);
}

@media (prefers-reduced-motion: reduce) {
  .marquee-stack {
    -webkit-mask-image: none;
    mask-image: none;
  }

  .marquee-row {
    overflow: visible;
    padding: 0;
  }

  .marquee-track {
    width: 100%;
    animation: none;
  }

  .marquee-group {
    width: 100%;
    padding-right: 0;
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(176px, 1fr));
    gap: 12px;
  }

  .marquee-group-dup {
    display: none;
  }

  .platform-card-marquee {
    width: auto;
  }

  .platform-card,
  .platform-dot,
  .platform-arrow {
    transition: none;
    transform: none;
  }
}
</style>
