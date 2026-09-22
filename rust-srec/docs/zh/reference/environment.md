<script setup>
import { withBase } from 'vitepress'
</script>

# 环境变量 {#环境变量}

你可以在 <a :href="withBase('/env.zh.example')" download=".env.example">.env</a> 文件中配置以下环境变量。

## 通用 {#通用}
| 变量 | 说明 | 默认值 |
|------|------|--------|
| `TZ` | 容器时区 | `UTC` (建议 `Asia/Shanghai`) |
| `VERSION` | Docker 镜像版本标签 | `latest` |

## 路径 {#路径}
| 变量 | 说明 | 默认值 |
|------|------|--------|
| `DATA_DIR` | 应用数据目录 | `./data` |
| `CONFIG_DIR` | 平台配置文件目录 | `./config` |
| `OUTPUT_DIR` | 独立后端创建全新数据库时的初始录制文件夹，同时供启动和磁盘空间健康探测监视。在 Docker Compose 中它是宿主机绑定挂载目录，容器内使用 `OUTPUT_DIR=/app/output`。已有数据库设置会保留。 | `./output` |
| `LOG_DIR` | 日志文件目录。相对路径按进程工作目录解析；随附的系统服务单元会显式设为 `/var/log/rust-srec`，以免日志文件落进状态目录。参见[安装](../getting-started/installation.md)。 | `./logs` |
| `LOG_MAX_FILE_BYTES` | 启动时读取的单个托管日志分段字节上限，整数范围 1024 至 1073741824；超大记录截断并添加标记。 | `16777216`（16 MiB） |
| `LOG_MAX_FILES` | 启动时读取的托管日志分段数量上限，含当前文件，整数范围 2 至 1024。共享 `LOG_DIR` 的实例应保持设置一致；参见[保留上限](../operations/monitoring.md#日志)。 | `16` |

::: tip 初始录制目录与已保存的录制目录
独立后端使用 `OUTPUT_DIR` 初始化全新数据库的 `output_folder`，未设置或为空白时使用 `./output`。相对路径按启动工作目录解析并保存为绝对路径。Docker Compose 和 systemd unit 分别提供 `/app/output` 和 `/var/lib/rust-srec/output`。

如果初始迁移或输出文件夹保存失败，下次启动会使用首次尝试时选定的绝对路径继续初始化，即使 `OUTPUT_DIR` 或工作目录已改变。

之后启动时保留已保存的设置。如需更改，请编辑 **设置** → **全局** → **输出文件夹**，也可按平台、模板和主播分别覆盖。应以应用显示的解析后路径为准。已有二进制或系统服务安装若仍使用 `/app/output`，需要将该设置改为可写目录。

配置显式边界时，请让 `RUST_SREC_OUTPUT_ROOTS` 与已保存的文件夹保持一致。探测发现使用已保存的输出设置和覆盖项；初始化后，过时的 `OUTPUT_DIR` 值不会额外增加一个探测位置。
:::

## 关闭 {#关闭}
| 变量 | 说明 | 默认值 |
|------|------|--------|
| `RUST_SREC_SHUTDOWN_TIMEOUT_SECS` | 独立后端进程的严格关闭期限 | `30` |
| `RUST_SREC_SHUTDOWN_FORCE_RESERVE_SECS` | 在期限内为强制终止进程树预留的时间；必须大于零且小于总期限 | `2` |
| `RUST_SREC_CONTAINER_STOP_GRACE_PERIOD` | Docker Compose 发送外部 SIGKILL 前的等待时间；必须长于后端期限 | `35s` |
| `RUST_SREC_RUNTIME_MARKER_PATH` | 强制终止或崩溃后保留的未清理运行世代标记 | 位于 SQLite 数据库旁边 |

独立后端默认允许 30 秒关闭，最后两秒预留给强制进程清理。通过 `RUST_SREC_SHUTDOWN_TIMEOUT_SECS` 调整总时限，强制预留需大于零且小于总时限。Docker 停止宽限期必须更长。

强制退出或崩溃会在数据库旁保留恢复标记，后续正常退出不会清除此前未解决的恢复状态。只有后端已停止且中断文件已核对后，才能删除标记。退出状态 `124` 表示硬期限耗尽，`125` 表示无法请求终止进程树。信号和进程清理细节见[运行时关闭](../development/architecture.md#可观测性、健康检查与优雅退出)。

## 网络 {#网络}
| 变量 | 说明 | 默认值 |
|------|------|--------|
| `API_BIND_ADDRESS` | 后端 API 绑定的 IP 地址 | `0.0.0.0` |
| `API_PORT` | 后端 API 的外部端口 | `12555` |
| `FRONTEND_PORT` | Web 界面的外部端口 | `15275` |
| `BACKEND_URL` | 前端访问后端的内部 URL | `http://rust-srec:8080` |
| `HTTP_PROXY` | HTTP 代理服务器 URL | - |
| `HTTPS_PROXY` | HTTPS 代理服务器 URL | - |
| `NO_PROXY` | 绕过代理的主机列表（逗号分隔） | - |

## 安全与认证 {#安全与认证}
| 变量 | 说明 | 默认值 |
|------|------|--------|
| `JWT_SECRET` | JWT 签名密钥（**必需**，除非使用下述仅限本地的关闭选项） | - |
| `AUTH_DISABLED` | 仅在绑定到回环地址的本地开发环境中关闭后端认证 | `false` |
| `API_CORS_ORIGINS` | 关闭认证时，允许跨域调用 API 的浏览器来源列表（逗号分隔的精确 `scheme://host[:port]`） | 本地开发服务器与桌面端 Webview 来源 |
| `API_LOGIN_MAX_FAILURES` | 单个账号在窗口内允许的登录失败次数 | `5` |
| `API_LOGIN_IP_MAX_FAILURES` | 单个来源地址在窗口内允许的登录失败次数 | `100` |
| `API_LOGIN_WINDOW_SECS` | 登录失败统计窗口长度（秒） | `900`（15 分钟） |
| `JWT_ISSUER` | JWT 签发者标识 | `rust-srec` |
| `JWT_AUDIENCE` | JWT 受众标识 | `rust-srec-api` |
| `SESSION_SECRET` | 前端会话加密密钥 (**必需**, 至少 32 位) | - |
| `COOKIE_SECURE` | 设置为 `true` 以强制仅 HTTPS Cookie | (自动) |
| `MIN_PASSWORD_LENGTH` | 用户密码最小长度 | `8` |

后端在未配置非空 `JWT_SECRET` 时会拒绝启动。仅在本地开发时，可以同时设置 `AUTH_DISABLED=true` 和 `API_BIND_ADDRESS=127.0.0.1`（或 `::1`）来关闭认证。通配地址、主机名和非回环绑定地址均不能使用此关闭选项。

关闭认证时，只有 `API_CORS_ORIGINS` 中列出的来源可以从浏览器跨域调用 API；默认列表包含 `http://localhost:15275`、`http://127.0.0.1:15275`、`http://[::1]:15275`、`tauri://localhost` 和 `http://tauri.localhost`。设置该变量可覆盖默认值——每一项必须是不带路径的精确来源，格式错误的条目会在启动时记录警告并被忽略。来自其他来源的请求会被拒绝并返回 `403`；`Host` 请求头既不是回环名称也不是所配置绑定地址的请求同样会被拒绝。启用认证时该变量不生效，任何来源都可以发起请求，因为受保护路由仍然需要 Bearer 令牌。

## 登录限流 {#登录限流}

`POST /api/auth/login` 会在滑动窗口内统计失败次数，配额用尽后返回 `429` 并在 `Retry-After` 中给出等待时间。每次尝试同时受两个配额约束：

- **按账号**（`API_LOGIN_MAX_FAILURES`，默认 5）。登录成功会立即清零。
- **按来源地址**（`API_LOGIN_IP_MAX_FAILURES`，默认 100）。这个配额刻意放得很宽：来源地址取自 TCP 连接的对端，且不信任 `X-Forwarded-For`，因此在本项目自带的前端容器、nginx 或任何反向代理之后，**所有登录都来自代理的地址**。请把它理解为对密码哈希开销的上限，而不是针对某个用户的锁定——在配额用尽期间，该代理之后的所有用户都会被限流。如果这一点比哈希开销上限更重要，可以调大；只有在浏览器直连后端时才建议调低。

两者共用 `API_LOGIN_WINDOW_SECS` 设置的窗口长度。

## 令牌过期 {#令牌过期}
| 变量 | 说明 | 默认值 |
|------|------|--------|
| `ACCESS_TOKEN_EXPIRATION_SECS` | JWT 访问令牌有效期 | `3600` (1h) |
| `REFRESH_TOKEN_EXPIRATION_SECS` | JWT 刷新令牌有效期 | `604800` (7d) |

## 浏览器通知 (Web Push / VAPID) {#浏览器通知-web-push-vapid}
| 变量 | 说明 | 默认值 |
|------|------|--------|
| `WEB_PUSH_VAPID_PUBLIC_KEY` | VAPID 公钥 (base64url, 无 padding)。留空/不设置则禁用。 | - |
| `WEB_PUSH_VAPID_PRIVATE_KEY` | VAPID 私钥 (base64url, 无 padding)。留空/不设置则禁用。 | - |
| `WEB_PUSH_VAPID_SUBJECT` | VAPID subject（例如 `mailto:admin@localhost`） | `mailto:admin@localhost` |

## 后端服务 {#后端服务}
| 变量 | 说明 | 默认值 |
|------|------|--------|
| `RUST_LOG` | 日志级别 (`trace`, `debug`, `info`, `warn`, `error`) | `info` |
| `DATABASE_URL` | SQL 数据库连接字符串。表中所列是 Docker `.env` 设置的值。未设置时后端回退到 `sqlite:srec.db?mode=rwc`，相对工作目录解析；随附的系统服务单元则设为 `sqlite:///var/lib/rust-srec/rust-srec.db`。运行世代标记由该 URL 推导，与数据库文件放在一起。 | `sqlite:///app/data/rust-srec.db`（Docker） |
| `RUST_SREC_LOCALE` | 后端通知字符串的语言环境。影响所有通知事件——直播上/下线、录制生命周期、分段、流水线任务、系统告警、凭据事件。支持：`en`、`zh-CN`。 | `en` |
| `RUST_SREC_OUTPUT_ROOTS` | 以逗号分隔的**绝对**路径列表，作为写入门（write gate）的输出根边界。未设置时，写入门会对每个解析后的输出路径取前**两段有名分量**作为默认（例如 `/rec/huya/X/20260415` → `/rec/huya`，`/home/user/recordings/X/20260415` → `/home/user`）。两段是最小安全默认值——它可以避免意外将 `/home/...` 布局下不同用户合并到同一个门键。如果您是 `/rec` 这种单挂载布局，且希望一个挂载点对应一个门键（从而在故障时只收到一条聚合通知、而不是按平台分别通知），请显式设置：`RUST_SREC_OUTPUT_ROOTS=/rec`。 | - |

默认规则会把深层路径归入较宽的键：`/var/lib/rust-srec/output` 使用 `/var/lib`。启动发现会测试映射到同一键的具体录制目录，因此不要求对只读祖先目录具有写权限。设置 `RUST_SREC_OUTPUT_ROOTS=/var/lib/rust-srec/output` 可为该目录指定独立边界；最长匹配的显式前缀优先。显式边界本身也会被探测，应指向可写的录制位置。发现范围的限制详见[输出根探测](../operations/storage.md#输出根探测)。

## 资源限制 (Docker) {#资源限制-docker}
| 变量 | 说明 | 默认值 |
|------|------|--------|
| `CPU_LIMIT` | 容器可使用的最大 CPU 核心数 | `4` |
| `MEMORY_LIMIT` | 容器可使用的最大内存 | `4G` |
| `CPU_RESERVATION` | 容器保留的 CPU 核心数 | `1` |
| `MEMORY_RESERVATION` | 容器保留的内存 | `512M` |
