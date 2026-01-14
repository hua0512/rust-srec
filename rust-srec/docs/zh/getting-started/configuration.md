<script setup>
import { withBase } from 'vitepress'
</script>

# 配置

rust-srec 使用 **4 层配置层级** 实现灵活控制。详见 [配置层级](../concepts/configuration.md)。

## 基础配置

### 添加第一个主播

1. 打开前端 http://localhost:15275
2. 使用默认凭据登录：
   - **用户名**：`admin`
   - **密码**：`admin123!`
3. 进入 **主播** → **添加主播**
4. 输入：
   - **名称**：显示名称
   - **URL**：直播间地址（如 `https://www.bilibili.com/xxxx`）
   - **平台**：根据 URL 自动识别
5. 点击 **保存**

### 全局设置

通过 **设置** → **全局配置** 访问。设置项分为以下几类：

#### 文件配置 (File Configuration)
| 设置 | 说明 | 默认值 |
|------|------|--------|
| `record_danmu` | 启用弹幕录制 | `false` |
| `auto_thumbnail` | 自动生成视频封面 | `true` |
| `output_folder` | 录制保存的基础目录（支持模板） | `/app/output` |
| `output_filename_template` | 录制文件的文件名模板 | (见下文) |
| `output_file_format` | 默认输出格式 (mp4, flv 等) | `flv` |

#### 资源限制 (Resource Limits)
| 设置 | 说明 | 默认值 |
|------|------|--------|
| `min_segment_size` | 保留分段的最小大小 | `1MB` |
| `max_download_duration_secs` | 分段的最大时长 | `0` (不限制) |
| `max_part_size` | 分段的最大大小 | `8GB` |

#### 并发与性能 (Concurrency & Performance)
| 设置 | 说明 | 默认值 |
|------|------|--------|
| `max_concurrent_downloads` | 最大同时录制任务数 | `6` |
| `max_concurrent_uploads` | 最大同时上传任务数 | `3` |
| `max_cpu_jobs` | 最大并发 CPU 密集型任务数 | `0` (Auto / 自动) |
| `max_io_jobs` | 最大并发 I/O 密集型任务数 | `8` (0 = Auto / 自动) |
| `download_engine` | 录制引擎 (`ffmpeg`, `mesio` 等) | `mesio` |

#### 网络与系统 (Network & System)
| 设置 | 说明 | 默认值 |
|------|------|--------|
| `streamer_check_interval` | 检查主播状态的间隔 | `60 Secs` |
| `offline_check_interval` | 检查离线状态的间隔 | `20 Secs` |
| `offline_detection_count` | 判定主播离线前的重试次数 | `3` |
| `retention_period` | 历史记录保留天数 | `30 Days` |
| `session_gap_time_secs` | 判定会话结束的等待时间 | `1 Hour` |
| `enable_proxy` | 通过代理服务器路由流量 | `false` |

#### 流水线配置 (Pipeline Configuration)
Rust-Srec 拥有强大的模块化流水线系统，可以在不同阶段添加自定义步骤（如：转码、通知、自定义脚本）：
- **Per-segment (分段后)**: 在每个视频分段录制完成后立即运行。
- **Paired Segment (合并对)**: 在视频和弹幕配对后运行。
- **Session Complete (会话结束)**: 在整个录制会话结束后运行。

::: info 目录组织
将 `output_folder` 设置为 `{streamer}/%Y-%m-%d` 可按主播分类并按日期建立子文件夹。`output_filename_template` 则可使用 `%H-%M-%S_{title}` 作为文件名。
:::

## 环境变量

你可以在 <a :href="withBase('/env.zh.example')" download=".env.example">.env</a> 文件中配置以下环境变量。

### 通用
| 变量 | 说明 | 默认值 |
|------|------|--------|
| `TZ` | 容器时区 | `UTC` (建议 `Asia/Shanghai`) |
| `VERSION` | Docker 镜像版本标签 | `latest` |

### 路径
| 变量 | 说明 | 默认值 |
|------|------|--------|
| `DATA_DIR` | 应用数据目录 | `./data` |
| `CONFIG_DIR` | 平台配置文件目录 | `./config` |
| `OUTPUT_DIR` | 录制文件存储目录 | `/app/output` |
| `LOG_DIR` | 日志文件目录 | `./logs` |

### 网络
| 变量 | 说明 | 默认值 |
|------|------|--------|
| `API_PORT` | 后端 API 的外部端口 | `12555` |
| `FRONTEND_PORT` | Web 界面的外部端口 | `15275` |
| `BACKEND_URL` | 前端访问后端的内部 URL | `http://rust-srec:8080` |
| `HTTP_PROXY` | HTTP 代理服务器 URL | - |
| `HTTPS_PROXY` | HTTPS 代理服务器 URL | - |
| `NO_PROXY` | 绕过代理的主机列表（逗号分隔） | - |

### 安全与认证
| 变量 | 说明 | 默认值 |
|------|------|--------|
| `JWT_SECRET` | JWT 签名密钥 (**必需**) | - |
| `JWT_ISSUER` | JWT 签发者标识 | `rust-srec` |
| `JWT_AUDIENCE` | JWT 受众标识 | `rust-srec-api` |
| `SESSION_SECRET` | 前端会话加密密钥 (**必需**, 至少 32 位) | - |
| `COOKIE_SECURE` | 设置为 `true` 以强制仅 HTTPS Cookie | (自动) |
| `MIN_PASSWORD_LENGTH` | 用户密码最小长度 | `8` |

### 令牌过期
| 变量 | 说明 | 默认值 |
|------|------|--------|
| `ACCESS_TOKEN_EXPIRATION_SECS` | JWT 访问令牌有效期 | `3600` (1h) |
| `REFRESH_TOKEN_EXPIRATION_SECS` | JWT 刷新令牌有效期 | `604800` (7d) |

### 后端服务
| 变量 | 说明 | 默认值 |
|------|------|--------|
| `RUST_LOG` | 日志级别 (`trace`, `debug`, `info`, `warn`, `error`) | `info` |
| `DATABASE_URL` | SQL 数据库连接字符串 | `sqlite:///app/data/rust-srec.db` |

### 资源限制 (Docker)
| 变量 | 说明 | 默认值 |
|------|------|--------|
| `CPU_LIMIT` | 容器可使用的最大 CPU 核心数 | `4` |
| `MEMORY_LIMIT` | 容器可使用的最大内存 | `4G` |
| `CPU_RESERVATION` | 容器保留的 CPU 核心数 | `1` |
| `MEMORY_RESERVATION` | 容器保留的内存 | `512M` |

## 文件名模板变量

Rust-Srec 支持在 `output_folder` 和 `output_filename_template` 中使用两类占位符。

### 大括号变量 (Curly Brace Variables)
这些变量将被替换为主播或会话相关的元数据。

| 变量 | 说明 |
|------|------|
| `{streamer}` | 主播显示名称 |
| `{title}` | 当前直播标题 |
| `{platform}` | 平台名称 (如 bilibili) |
| `{session_id}` | 录制会话的唯一 ID (仅适用于 `output_folder`) |

### 百分号占位符 (Percent Placeholders, FFmpeg 风格)
这些占位符将被替换为日期、时间或序列信息。

| 占位符 | 说明 |
|--------|------|
| `%Y` | 年份 (YYYY) |
| `%m` | 月份 (01-12) |
| `%d` | 日期 (01-31) |
| `%H` | 小时 (00-23) |
| `%M` | 分钟 (00-59) |
| `%S` | 秒数 (00-59) |
| `%i` | 分段序列号 |
| `%t` | Unix 时间戳 |
| `%%` | 字面量百分号 |

示例：`{streamer}/%Y-%m-%d/%H-%M-%S_{title}`
