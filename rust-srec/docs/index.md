---
layout: page
---

<script setup>
import { onMounted } from 'vue'
import { useRouter, withBase } from 'vitepress'

const { go } = useRouter()

onMounted(() => {
  go(withBase('/en/'))
})
</script>

<div style="display: flex; flex-direction: column; justify-content: center; align-items: center; height: 50vh; gap: 1rem;">
  <img src="/stream-rec-orange.svg" alt="rust-srec" style="width: 64px; height: 64px;" />
  <p>Redirecting...</p>
  <p><a href="./en/">Click here if not redirected</a></p>
</div>
