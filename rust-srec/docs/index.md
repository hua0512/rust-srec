---
layout: page
navbar: false
footer: false
search: false
---

<script setup lang="ts">
import { onMounted } from 'vue'
import { useRouter, withBase } from 'vitepress'

const { go } = useRouter()

onMounted(() => {
  const locale = navigator.languages?.[0] || navigator.language || 'en'
  go(withBase(locale.toLowerCase().startsWith('zh') ? '/zh/' : '/en/'))
})
</script>

<div class="lang-picker">
  <img :src="withBase('/stream-rec-orange.svg')" alt="Rust-Srec" class="lang-picker-logo" />
  <p class="lang-picker-title">Rust-Srec Documentation</p>
  <p class="lang-picker-subtitle">Choose a language / 选择语言</p>
  <div class="lang-picker-links">
    <a class="lang-picker-btn" :href="withBase('/en/')">English</a>
    <a class="lang-picker-btn" :href="withBase('/zh/')">简体中文</a>
  </div>
</div>
