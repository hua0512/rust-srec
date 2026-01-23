import { emit, listen } from '@tauri-apps/api/event';

type BootProgressPayload = {
  status?: string;
  progress?: number;
};

function clamp01(value: number): number {
  if (value < 0) return 0;
  if (value > 1) return 1;
  return value;
}

function updateProgress(payload: BootProgressPayload): void {
  const statusEl = document.getElementById('status');
  const progressBar = document.getElementById('progress-bar');
  const percentEl = document.getElementById('percent');

  if (statusEl && typeof payload.status === 'string' && payload.status.length > 0) {
    statusEl.textContent = payload.status;
  }

  if (typeof payload.progress === 'number' && Number.isFinite(payload.progress)) {
    const p = clamp01(payload.progress);
    if (progressBar) {
      (progressBar as HTMLElement).style.width = `${p * 100}%`;
    }
    if (percentEl) {
      percentEl.textContent = `${Math.round(p * 100)}%`;
    }
  }
}

async function bootstrapSplash(): Promise<void> {
  try {
    await listen<BootProgressPayload>('boot-progress', (event) => {
      if (event.payload) {
        updateProgress(event.payload);
      }
    });

    // Tell the native host we're ready so it can replay the latest progress state.
    await emit('rust-srec://splash-ready');
  } catch {
    // best-effort: splash should still render even if events are unavailable
  }
}

if (document.readyState === 'loading') {
  document.addEventListener('DOMContentLoaded', () => {
    void bootstrapSplash();
  });
} else {
  void bootstrapSplash();
}
