<script setup lang="ts">
import { withBase } from 'vitepress'

export interface DocCardItem {
  title: string
  description: string
  link: string
  badge?: string
  badgeType?: 'info' | 'tip' | 'warning'
}

const props = withDefaults(
  defineProps<{
    items: DocCardItem[]
    columns?: 2 | 3
  }>(),
  {
    columns: 2,
  },
)

function resolveHref(link: string): string {
  if (/^https?:\/\//i.test(link)) return link
  return withBase(link)
}

function isExternal(link: string): boolean {
  return /^https?:\/\//i.test(link)
}
</script>

<template>
  <div class="doc-card-grid" :class="`cols-${props.columns}`">
    <a
      v-for="item in props.items"
      :key="item.link + item.title"
      :href="resolveHref(item.link)"
      :target="isExternal(item.link) ? '_blank' : undefined"
      :rel="isExternal(item.link) ? 'noopener noreferrer' : undefined"
      class="doc-card"
    >
      <div class="doc-card-top">
        <span class="doc-card-title">{{ item.title }}</span>
        <span
          v-if="item.badge"
          class="doc-card-badge"
          :class="`badge-${item.badgeType ?? 'info'}`"
        >
          {{ item.badge }}
        </span>
      </div>
      <p class="doc-card-desc">{{ item.description }}</p>
      <span class="doc-card-arrow" aria-hidden="true">→</span>
    </a>
  </div>
</template>

<style scoped>
.doc-card-grid {
  display: grid;
  gap: 14px;
  margin: 20px 0;
}

.doc-card-grid.cols-2 {
  grid-template-columns: repeat(2, minmax(0, 1fr));
}

.doc-card-grid.cols-3 {
  grid-template-columns: repeat(3, minmax(0, 1fr));
}

.doc-card {
  position: relative;
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 18px 20px;
  border-radius: 12px;
  border: 1px solid var(--vp-c-divider);
  background: var(--vp-c-bg-soft);
  text-decoration: none !important;
  transition:
    border-color 0.2s ease,
    background-color 0.2s ease,
    transform 0.2s ease,
    box-shadow 0.2s ease;
}

.doc-card:hover {
  border-color: var(--vp-c-brand-1);
  background: var(--vp-c-bg-elv);
  transform: translateY(-2px);
  box-shadow: 0 10px 26px -14px rgba(228, 77, 38, 0.24);
}

.doc-card-top {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding-right: 20px;
}

.doc-card-title {
  font-size: 15px;
  font-weight: 600;
  color: var(--vp-c-text-1);
  line-height: 1.4;
}

.doc-card-badge {
  display: inline-flex;
  align-items: center;
  padding: 2px 8px;
  border-radius: 999px;
  font-size: 11px;
  font-weight: 600;
  white-space: nowrap;
}

.badge-info {
  background: var(--vp-badge-info-bg);
  color: var(--vp-badge-info-text);
}

.badge-tip {
  background: var(--vp-badge-tip-bg);
  color: var(--vp-badge-tip-text);
}

.badge-warning {
  background: var(--vp-badge-warning-bg);
  color: var(--vp-badge-warning-text);
}

.doc-card-desc {
  margin: 0 !important;
  font-size: 13.5px;
  line-height: 1.6;
  color: var(--vp-c-text-2);
  padding-right: 18px;
}

.doc-card-arrow {
  position: absolute;
  top: 18px;
  right: 18px;
  font-size: 15px;
  color: var(--vp-c-text-3);
  transition: transform 0.2s ease, color 0.2s ease;
}

.doc-card:hover .doc-card-arrow {
  color: var(--vp-c-brand-1);
  transform: translateX(3px);
}

@media (max-width: 820px) {
  .doc-card-grid.cols-3 {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }
}

@media (max-width: 600px) {
  .doc-card-grid.cols-2,
  .doc-card-grid.cols-3 {
    grid-template-columns: 1fr;
  }
}

@media (prefers-reduced-motion: reduce) {
  .doc-card,
  .doc-card-arrow {
    transition: none;
    transform: none;
  }
}
</style>
