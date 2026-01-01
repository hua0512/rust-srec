---
layout: page
---

<script setup>
import { withBase } from 'vitepress'

if (typeof window !== 'undefined') {
  window.location.href = withBase('/en/')
}
</script>

<div style="display: flex; flex-direction: column; justify-content: center; align-items: center; height: 50vh; gap: 1rem;">
  <img :src="withBase('/stream-rec-white.svg')" alt="rust-srec" style="width: 64px; height: 64px;" />
  <p>Redirecting...</p>
  <p><a :href="withBase('/en/')">Click here if not redirected</a></p>
</div>
