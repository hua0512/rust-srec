<script setup>
import { withBase } from 'vitepress'
</script>

# Docker 部署

标准部署在一台 Docker 主机上运行后端和前端，并通过绑定挂载持久化数据。

## 前置要求

- [Docker Engine 或 Docker Desktop](https://docs.docker.com/get-docker/)
- [Docker Compose v2](https://docs.docker.com/compose/install/)，命令格式为 `docker compose`
- 满足录制容量的持久化存储

## 安装脚本

Linux 与 macOS：

```bash
curl -fsSL https://docs.srec.rs/install.sh | SREC_LANG=zh bash
```

Windows PowerShell：

```powershell
$env:SREC_LANG = "zh"; irm https://docs.srec.rs/install.ps1 | iex
```

安装脚本会下载 Compose 和环境文件、生成唯一密钥、检测可选的 NVIDIA 支持，并询问是否启动服务。两个入口都会按 `SREC_LANG` 或系统语言选择对应语言的安装脚本；上面的命令显式指定了 `zh`，因此系统语言不是中文时也会使用中文安装脚本。

::: warning 审查远程脚本
把远程响应直接传给 Shell 会执行网络上当时返回的内容。有变更控制要求的环境应先下载脚本、审查内容和校验值，再运行已审核的本地副本，或采用下面的手动安装。
:::

安装参数：

| 变量 | 用途 | 默认值 |
|---|---|---|
| `SREC_LANG` | `en` 或 `zh` | 按系统语言检测 |
| `RUST_SREC_DIR` | 安装目录 | `./rust-srec` |
| `VERSION` | 镜像标签 | `latest` |

使用已审查发布版时应设置准确版本。Linux/macOS 示例：

```bash
curl -fsSL https://docs.srec.rs/install.sh | SREC_LANG=zh RUST_SREC_DIR=/opt/rust-srec VERSION=v0.5.1 bash
```

::: warning 与 systemd 服务的路径冲突
[systemd 服务](./installation.md#systemd-服务-linux)会把后端二进制安装到 `/opt/rust-srec/rust-srec`。两种部署共用一台主机时，请改用其他 `RUST_SREC_DIR`。
:::

## 手动安装

1. 创建目录并下载两个文件：

   - <a :href="withBase('/docker-compose.example.yml')" download>docker-compose.example.yml</a>
   - <a :href="withBase('/env.zh.example')" download>.env.example</a>

2. 分别重命名为 `docker-compose.yml` 和 `.env`。
3. 生成两个不同密钥，填写 `.env` 中的 `JWT_SECRET` 和 `SESSION_SECRET`。空值会让 Compose 有意拒绝启动。
4. 审查目录、时区、端口和 `VERSION`。
5. 启动并验证：

```bash
docker compose up -d
docker compose ps
curl http://localhost:12555/api/health/live
```

可用 `openssl rand -hex 32` 生成密钥；没有 openssl 时可用 `python3 -c 'import secrets; print(secrets.token_hex(32))'`。PowerShell 命令：

```powershell
$bytes = New-Object Byte[] 32
[Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($bytes)
-join ($bytes | ForEach-Object { "{0:x2}" -f $_ })
```

## 默认布局

| 宿主机 | 容器 | 用途 |
|---|---|---|
| `./data` | `/app/data` | SQLite 与应用数据 |
| `./config` | `/app/config` | 平台配置 |
| `./output` | `/app/output` | 录制和管道输出 |
| `./logs` | `/app/logs` | 应用日志 |
| `12555` | 后端 `8080` | API 与 Swagger |
| `15275` | 前端 `80` | Web 界面 |

长期部署应使用绝对宿主机路径。示例已经配置 `unless-stopped`、后端健康检查、前端启动顺序和容器日志轮转。

## 访问与首次登录

- Web 界面：`http://localhost:15275`
- Swagger UI：`http://localhost:12555/api/docs`
- 初始账号：`admin` / `admin123!`

首次登录必须修改密码，然后继续[完成第一次录制](./first-recording.md)。

## 可选配置

### 代理

在 `.env` 设置 `HTTP_PROXY`、`HTTPS_PROXY` 和 `NO_PROXY`；示例已把变量传给后端。然后开启**全局设置 > 下载器 > 代理 > 使用系统代理**。`NO_PROXY` 应包含 `localhost,127.0.0.1,rust-srec`。

### 浏览器推送

生成 VAPID 密钥、写入 `.env` 并重启后端。localhost 以外使用 Web Push 必须有 HTTPS。参见[通知系统](../concepts/notifications.md#web-push)。

### NVIDIA GPU

安装宿主机驱动和 [NVIDIA Container Toolkit](https://docs.nvidia.com/datacenter/cloud-native/container-toolkit/latest/install-guide.html)，下载 <a :href="withBase('/docker-compose.gpu.yml')" download>docker-compose.gpu.yml</a>，然后执行：

```bash
docker compose -f docker-compose.yml -f docker-compose.gpu.yml up -d
docker exec rust-srec nvidia-smi
```

容器能访问 GPU 仅代表兼容处理器可用；还需在管道中选择 NVENC 处理器，并监控**系统健康**中的 GPU 组件。

## 清理存储

应删除挂载输出目录内部的文件；容器运行期间不要重命名、替换或移动作为绑定挂载源的宿主机目录。把根目录移动到回收站的文件管理器也可能让现有容器继续引用已孤立目录。

系统健康显示输出根 `not_found` 时，确认宿主机目录后重启容器以重建挂载命名空间。显示 `storage_full` 时，只释放空间而不要替换目录；写入门可在后续探测恢复。参见[存储与容量](../operations/storage.md)和[常见问题](./faq.md#清理磁盘后录制仍未恢复)。

## 停止、更新与移除

```bash
# 停止并保留数据
docker compose stop

# 再次启动
docker compose up -d
```

修改 `VERSION` 前先阅读[升级与回滚](../operations/upgrading.md)。`docker compose down` 会移除容器和网络，但保留绑定挂载的宿主机数据；删除任何宿主机目录前必须核对准确路径。

面向互联网的主机应完成[生产部署](../operations/production.md)，不要直接公开示例端口。
