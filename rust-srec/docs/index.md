---
layout: page
---

<script setup>
import { onMounted } from 'vue'
import { useRouter, withBase } from 'vitepress'

const { go } = useRouter()

onMounted(() => {
  const locale = navigator.languages?.[0] || navigator.language || 'en'
  go(withBase(locale.toLowerCase().startsWith('zh') ? '/zh/' : '/en/'))
})
</script>

<div style="display: flex; flex-direction: column; justify-content: center; align-items: center; height: 50vh; gap: 1rem;">
  <img src="/stream-rec-orange.svg" alt="rust-srec" style="width: 64px; height: 64px;" />
  <p>Choose a language / 选择语言</p>
  <p><a href="./en/">English</a> · <a href="./zh/">简体中文</a></p>
</div>
