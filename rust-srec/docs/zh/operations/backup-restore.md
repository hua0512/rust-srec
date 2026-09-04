# 备份与恢复

应同时采用两层备份：配置导出便于迁移，而要恢复运行历史和媒体文件则必须进行文件系统快照。

## 各类备份包含什么

| 备份 | 包含 | 不包含 |
|---|---|---|
| **设置 > 备份与恢复**导出 | 全局设置、模板、主播及过滤器、引擎、平台设置、通知渠道/订阅、任务与管道预设、用户和密码哈希 | 录制媒体、会话/任务历史、日志、刷新令牌会话 |
| 文件系统备份 | 实际复制的 `DATA_DIR`、`CONFIG_DIR`、`OUTPUT_DIR`，以及可选 `LOG_DIR` | 外部上传目标和通知服务中的数据 |

::: warning 敏感导出
配置导出可能包含平台 Cookie、通知凭据、渠道设置、用户信息和密码哈希。必须加密、限制访问，且不要附加到公开 Issue。
:::

## 配置导出

在 Web 界面打开**设置 > 备份与恢复**并下载导出文件。对应 API 为 `GET /api/config/backup/export` 和 `POST /api/config/backup/import`。

导入支持两种模式：

- `merge` 更新匹配实体，并保留文件中没有的现有实体。
- `replace` 删除导入文件未包含的现有受管配置。该操作具有破坏性，应先在临时实例验证。

配置导出适合迁移；移除密钥后也可用于版本审查，但它不是数据库备份。

## 一致的文件系统备份

标准 Docker 布局可按以下流程操作：

1. 下载一份最新配置导出。
2. 禁止新录制任务或安排维护窗口。
3. 执行 `docker compose stop`，确保 SQLite 和活跃媒体文件一致。
4. 快照或复制 `DATA_DIR`、`CONFIG_DIR`、`OUTPUT_DIR`、`.env` 和 `docker-compose.yml`。只有事件保留政策要求时才备份 `LOG_DIR`。
5. 执行 `docker compose up -d`，并验证存活检查。

systemd 服务的流程相同，只是换成 unit 自己的路径：

1. 下载一份最新配置导出。
2. 禁止新录制任务或安排维护窗口。
3. 执行 `systemctl stop rust-srec`，确保 SQLite 和活跃媒体文件一致。
4. 快照或复制 `/var/lib/rust-srec`（数据库、WAL 文件和默认输出目录）、`/etc/rust-srec/rust-srec.env`，以及 `ReadWritePaths=` 中列出的录制卷。只有事件保留政策要求时才备份 `/var/log/rust-srec`。
5. 执行 `systemctl start rust-srec`，并验证存活检查。

`.env`、`/etc/rust-srec/rust-srec.env` 和备份媒体的保护强度不得低于在线服务。至少保留一份主机外备份，并定期校验完整性。

## 恢复演练

1. 准备空间充足的干净主机，并使用同一固定 Rust-Srec 版本。
2. 保持服务停止，把目录恢复到相同绝对路径，并恢复所有权和权限。
3. 恢复 `.env` 与 Compose 配置，或恢复 `/etc/rust-srec/rust-srec.env` 与 unit 文件。等价恢复时不要重新生成 `JWT_SECRET`，除非有意让全部访问令牌失效。
4. 启动服务并检查 `/api/health/live`。
5. 登录后检查带认证的 `/api/health/ready`，核对主播和会话，并播放或校验代表性媒体。
6. 用一个非关键频道测试录制和管道。

会话可以比它所属的主播存在得更久：`live_sessions.streamer_id` 允许为空，删除主播时会保留会话、媒体记录和会话上保存的主播名。因此恢复出的数据库中出现找不到对应主播的会话是正常现象，并不说明恢复有问题。

记录恢复耗时和实际丢失的数据时间范围；这才是你的真实 RTO 和 RPO。
