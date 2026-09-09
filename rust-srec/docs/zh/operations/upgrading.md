# 升级与回滚

后端与前端应配套升级。数据库迁移在启动时执行且不可回退，因此升级前的快照就是回滚边界。

## 升级前

1. 阅读当前版本到目标版本之间的全部发布说明。
2. 确认 `rust-srec` 和 `rust-srec-frontend` 都存在目标镜像标签。
3. 下载配置导出，并一致备份数据库、配置、输出、`.env` 和 Compose 文件。
4. 记录当前镜像标签或摘要，以及健康检查结果。
5. 避开活跃录制和管道任务安排窗口；被中断的媒体写入可能需要人工检查。

## Docker 升级

编辑 `.env`，把 `VERSION` 设置为已审查的目标版本，例如：

```dotenv
VERSION=v0.5.1
```

然后执行：

```bash
docker compose pull
docker compose up -d
docker compose ps
docker compose logs --tail=200 rust-srec
curl http://localhost:12555/api/health/live
```

登录后验证带认证的就绪检查、系统健康页、主播数量、最近会话、一次平台检查和一个非关键录制/管道。观察期结束前保留升级前备份。

## systemd 升级

原地替换二进制并重启 unit。`install` 是替换文件而不是写入原文件，因此重启之前正在运行的进程不受影响：

```bash
install -D -m 0755 rust-srec /opt/rust-srec/rust-srec
systemctl restart rust-srec
systemctl status rust-srec
journalctl -u rust-srec --since "5 min ago"
curl http://localhost:12555/api/health/live
```

只有当某个版本修改了 unit 文件本身时，才需要把 `rust-srec.service` 重新安装到 `/etc/systemd/system/` 并执行 `systemctl daemon-reload`；出现这种情况时发布说明会写明。之后按 Docker 升级同样的项目做升级后检查。

## 自动更新（Watchtower）

Watchtower 属于 Docker 部署，对 systemd 服务无效；systemd 服务按上文手动升级。

Compose 文件内置了一个可选的 `watchtower` 服务，可自动拉取新镜像并重建容器。默认不启用，通过以下命令开启：

```bash
docker compose --profile autoupdate up -d
```

只有在系统空闲时才会执行更新。停止容器前，Watchtower 会先运行 pre-update 钩子，钩子调用无需认证的 `GET /api/health/idle` 端点：

- `200` —— 没有正在录制或排队录制的任务，也没有执行中的管道任务（上传、remux、弹幕转换等），更新继续。
- `503` 或无响应 —— 钩子以退出码 75 结束，Watchtower 跳过本轮，等下一次轮询（`WATCHTOWER_POLL_INTERVAL`，默认 3600 秒）再试。

排队中（PENDING）的管道任务不会阻塞更新；它们已持久化，重启后会自动继续执行。前端容器使用同样的空闲门控，因此前后端镜像会在同一轮里一起更新。

注意事项：

- 自动更新要求使用可变镜像标签：保持 `VERSION=latest`（或 `dev`）。固定的 `vX.Y.Z` 标签不会收到更新。
- 数据库迁移仍在启动时执行且不可回退。自动更新跳过了手动升级前快照这一步，因此请保持定期备份（见[备份与恢复](./backup-restore.md)）并定期阅读发布说明。如果升级必须先经过审查，请继续使用固定标签的手动升级。
- 空闲检查在容器停止前一刻执行；在这几秒钟内刚开始的录制仍会被中断。

## 预设和模板时间戳

迁移 `20260907120000` 将 `job_presets`、`pipeline_presets` 和 `template_config` 中的
`created_at`、`updated_at` 统一为 Unix 整数毫秒。已有整数保持不变；历史日期字符串会按
其时区转换，不足一毫秒的精度会截断。API 日期仍使用字符串。

如果启动时提示 `Cannot normalize preset/template timestamps`，说明其中一列含有无效或
不受支持的历史值，例如旧版时间戳解码错误产生的超长年份日期。迁移会回滚并保留原始值。
请停止后端并保留数据库备份，检查这六列，仅在能够通过备份或其他记录确认正确时间时修复
对应值，然后重新启动以重试迁移。不要用当前时间替换无法确认的日期，也不要修改已发布的
迁移文件。

## 过滤器时区兼容性

迁移 `20260909120000` 会在已有 TimeBased 对象配置省略时区或将其设为 null 时，
写入 `"timezone":"local"`，保留原有录制时间安排。显式时区、Cron 规则、行标识和
其他配置成员保持不变。格式错误或非对象 JSON 不作修改；该迁移不修复历史无效配置。

新建规则省略时区或设为 null 时使用 UTC。现有 TimeBased 编辑请求省略该成员时保留
已存储时区；显式 null 表示 UTC。备份导出使用 `0.1.8` 格式及显式时区，旧版备份中
省略时区的 TimeBased 规则按本地时间导入。详见[过滤器时区](../concepts/configuration.md#过滤器时区与边界)。

## 回滚

除非发布说明明确兼容，否则不要让旧版二进制直接打开已被新版迁移的数据库。

1. 停止前后端服务。
2. 恢复升级前数据库和配置快照。
3. 把 `VERSION` 恢复为上一标签或镜像摘要，或把上一版本的二进制重新安装到 `/opt/rust-srec/rust-srec`。
4. 启动两个服务并重复健康与录制检查。

快照后创建的会话和配置会丢失。如这些变更重要，应单独保留失败升级的数据用于取证恢复，不要直接合并回已恢复数据库。

## 源码或二进制部署

把准确的后端二进制、前端构建、环境文件和数据库快照作为同一个发布单元保存。构建时使用 `--locked`，不要任意组合不同版本的前后端。
