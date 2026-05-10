-- Add a runtime-tunable knob for the GPU health probe cadence.
--
-- The GpuHealthMonitor (rust-srec/src/metrics/gpu_health.rs) shells out to
-- nvidia-smi on this interval to detect host-side cgroup-driver wipes that
-- leave the container with /dev/nvidia* nodes but no CUDA access (issue
-- #555, typically triggered by `systemctl daemon-reload` on cgroup v2
-- hosts). When the probe transitions Healthy -> Unhealthy, the monitor
-- emits a single GpuUnavailable notification and flips the `gpu` row on
-- the System Health page so users see the failure before the next NVENC
-- pipeline job fails.
--
-- 30 seconds matches DEFAULT_GATE_COOLDOWN_SECS for the output-root write
-- gate, giving operators one cadence to remember. Values below 30 s are
-- discouraged via the UI but allowed down to 1 s for testing; a sub-second
-- cadence is rejected by the API validator because each probe is a
-- nvidia-smi fork+exec (~50-200 ms).
--
-- Hot-reloadable: config updates call GpuHealthMonitor::set_interval, so a
-- change applied via the global-config UI takes effect within at most the
-- previous interval and never requires a restart.

ALTER TABLE global_config
    ADD COLUMN gpu_health_probe_interval_secs INTEGER NOT NULL DEFAULT 30;
