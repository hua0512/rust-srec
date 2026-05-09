# 更新日志

## `unreleased`

- 新增:系统健康页面现在会跟踪 GPU 健康状态。如果容器失去对 GPU 的访问(NVIDIA Container Toolkit 在 cgroup v2 主机上的已知问题),您会立即收到通知,而不必等到下一个 remux 任务失败时才发现。探测间隔可在全局设置页面调整。
