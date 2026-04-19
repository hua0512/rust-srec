<script setup>
import { withBase } from 'vitepress'
</script>

# Docker 部署

Docker 是运行 Rust-Srec 最简单且推荐的方式。

## 前置要求

- [Docker](https://docs.docker.com/get-docker/)
- [Docker Compose](https://docs.docker.com/compose/install/)

## 快速开始

### 一键安装 (Linux/macOS)

运行以下命令自动安装配置 Rust-Srec：

```bash
curl -fsSL https://docs.srec.rs/docker-install-zh.sh | bash
```

### 一键安装 (Windows PowerShell)

```powershell
irm https://docs.srec.rs/install.ps1 | iex
```

脚本会自动完成：
- 下载配置文件
- 生成安全密钥
- 可选择立即启动应用

::: tip 自定义安装
脚本会自动检测系统语言并选择中文版或英文版。你也可以通过环境变量自定义安装：

**Linux/macOS:**
```bash
# 安装开发版到自定义目录
RUST_SREC_DIR=/opt/rust-srec VERSION=dev curl -fsSL https://docs.srec.rs/docker-install-zh.sh | bash
```

**Windows PowerShell:**
```powershell
# 安装开发版到自定义目录 (强制使用中文版)
$env:SREC_LANG = "zh"; $env:RUST_SREC_DIR = "C:\rust-srec"; $env:VERSION = "dev"; irm https://docs.srec.rs/install.ps1 | iex
```

| 变量 | 说明 | 默认值 |
|------|------|--------|
| `SREC_LANG` | 语言选择 (`zh` 或 `en`) | 自动检测 |
| `RUST_SREC_DIR` | 安装目录 | `./rust-srec` |
| `VERSION` | Docker 镜像标签 (`latest` 或 `dev`) | `latest` |
:::

### 手动安装

1. 创建项目目录：
   ```bash
   mkdir rust-srec && cd rust-srec
   ```

2. 下载示例配置文件：
   - <a :href="withBase('/docker-compose.example.yml')" download>docker-compose.example.yml</a>
   - <a :href="withBase('/env.example')" download=".env.example">.env.example</a> (英文) 或 <a :href="withBase('/env.zh.example')" download=".env.example">.env.zh.example</a> (中文)

3. 重命名文件：
   ```bash
   # Linux/macOS
   mv docker-compose.example.yml docker-compose.yml
   mv .env.example .env

   # Windows
   rename docker-compose.example.yml docker-compose.yml
   rename .env.example .env
   ```

4. **编辑 `.env`**：确保设置了安全的 `JWT_SECRET` 和 `SESSION_SECRET`（至少 32 个字符）。

::: tip 安全提示
你可以使用以下命令生成一个安全的随机字符串：
- **Linux/macOS**: `openssl rand -hex 32`
- **Windows (PowerShell)**: `$bytes = New-Object Byte[] 32; [Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($bytes); -join ($bytes | ForEach-Object { "{0:x2}" -f $_ })`
:::

5. 启动应用：
   ```bash
   docker-compose up -d
   ```

::: warning 磁盘空间
请确保 `DATA_DIR` 和 `OUTPUT_DIR` 挂载在有足够剩余空间的驱动器上。直播录制会非常迅速地消耗磁盘空间。
:::

## 配置说明

<a :href="withBase('/env.example')" download=".env.example">.env</a> 文件包含了所有必要的环境变量。

| 变量 | 说明 |
|------|------|
| `JWT_SECRET` | JWT 签名密钥 (**必需**) |
| `SESSION_SECRET` | 前端会话加密密钥 (**必需**) |

### 浏览器通知 (Web Push)

如需启用浏览器推送通知，请生成 VAPID 密钥并配置到 `.env`：

```bash
docker run --rm ghcr.io/hua0512/rust-srec:latest /app/rust-srec-vapid
# 或: npx --yes web-push generate-vapid-keys
```

| 变量 | 说明 |
|------|------|
| `WEB_PUSH_VAPID_PUBLIC_KEY` | VAPID 公钥 (base64url, 无 padding) |
| `WEB_PUSH_VAPID_PRIVATE_KEY` | VAPID 私钥 (base64url, 无 padding) |
| `WEB_PUSH_VAPID_SUBJECT` | VAPID subject（例如 `mailto:admin@localhost`） |

::: tip 提示
Web Push 需要 HTTPS（或 localhost）。
:::

::: tip 完整参考
有关所有可用环境变量及其说明的完整列表，请参阅 [环境变量参考](./configuration.md#environment-variables)。
:::

### docker-compose.yml

我们的标准示例 <a :href="withBase('/docker-compose.example.yml')" download>docker-compose.example.yml</a> 包含了：
- **自动重启**：`unless-stopped`
- **健康检查**：确保前端在后端就绪后才启动
- **资源限制**：可配置的 CPU 和内存限制
- **日志轮转**：防止日志撑爆磁盘

## 代理配置

如果您处于需要代理的环境中，可以为应用程序和下载引擎配置代理。

### 1. 配置环境变量

在 `.env` 文件中添加 `HTTP_PROXY` 和 `HTTPS_PROXY`：

```bash
# .env
HTTP_PROXY=http://your-proxy-host:port
HTTPS_PROXY=http://your-proxy-host:port
NO_PROXY=localhost,127.0.0.1,rust-srec
```

### 2. 更新 docker-compose.yml

确保这些变量传递给 `rust-srec` 服务：

```yaml
services:
  rust-srec:
    # ...
    environment:
      - HTTP_PROXY=${HTTP_PROXY:-}
      - HTTPS_PROXY=${HTTPS_PROXY:-}
      - NO_PROXY=${NO_PROXY:-}
    # ...
```

### 3. 在应用设置中启用

启动应用程序后，前往 **全局设置** > **下载器** > **代理**：
1. 启用 **代理 (Proxy)**。
2. 勾选 **使用系统代理 (Use System Proxy)**。
3. 保存设置。

这将指示应用程序及其引擎（FFmpeg、Streamlink、Mesio）遵循您配置的环境变量。

## GPU 硬件加速 (NVIDIA)

如果您拥有 NVIDIA GPU，可以启用硬件加速视频转码 (NVENC/NVDEC)，大幅降低 CPU 占用并加速转封装/转码。

### 前置要求

1. 主机已安装 **NVIDIA GPU 驱动**
2. 主机已安装 **NVIDIA Container Toolkit** — 允许 Docker 容器访问 GPU：
   - [安装指南](https://docs.nvidia.com/datacenter/cloud-native/container-toolkit/install-guide.html)
   - 快速安装 (Ubuntu/Debian)：
     ```bash
     curl -fsSL https://nvidia.github.io/libnvidia-container/gpgkey | sudo gpg --dearmor -o /usr/share/keyrings/nvidia-container-toolkit-keyring.gpg
     curl -s -L https://nvidia.github.io/libnvidia-container/stable/deb/nvidia-container-toolkit.list | \
       sed 's#deb https://#deb [signed-by=/usr/share/keyrings/nvidia-container-toolkit-keyring.gpg] https://#g' | \
       sudo tee /etc/apt/sources.list.d/nvidia-container-toolkit.list
     sudo apt-get update && sudo apt-get install -y nvidia-container-toolkit
     sudo nvidia-ctk runtime configure --runtime=docker
     sudo systemctl restart docker
     ```

### 在 docker-compose 中启用 GPU

下载 GPU compose 覆盖文件，放在 `docker-compose.yml` 同级目录：

- <a :href="withBase('/docker-compose.gpu.yml')" download>docker-compose.gpu.yml</a>

然后使用两个文件启动：

```bash
docker compose -f docker-compose.yml -f docker-compose.gpu.yml up -d
```

或者在 `.env` 中设置 `COMPOSE_FILE`，这样 `docker compose up -d` 会自动加载：

```bash
echo "COMPOSE_FILE=docker-compose.yml:docker-compose.gpu.yml" >> .env
```

覆盖文件会添加 NVIDIA 设备预留配置：

```yaml
services:
  rust-srec:
    deploy:
      resources:
        reservations:
          devices:
            - driver: nvidia
              count: 1
              capabilities: [gpu, video]
```

::: tip 自动配置
如果使用我们的[一键安装脚本](#一键安装-linux-macos)，脚本会自动检测 NVIDIA GPU 并提示您是否启用。
:::

### 验证 GPU 访问

启动容器后，验证 GPU 是否可访问：

```bash
docker exec rust-srec nvidia-smi
```

您应该能看到 GPU 型号和驱动版本。如果出现错误，说明 NVIDIA Container Toolkit 未正确配置。

### 在应用中启用

容器可访问 GPU 后，前往录制预设设置，启用**硬件加速**并选择 `cuda` 设备以使用 NVENC 编码。

### 常见问题排查

| 现象 | 原因 | 解决方法 |
|------|------|----------|
| ffmpeg 日志显示 `Cannot load libnvcuvid.so.1` | 容器无法访问 GPU 驱动 | 安装 NVIDIA Container Toolkit 并重启 Docker |
| 容器内找不到 `nvidia-smi` | Container Toolkit 未配置 | 运行 `sudo nvidia-ctk runtime configure --runtime=docker && sudo systemctl restart docker` |
| 已启用 GPU 但 CPU 占用仍然很高 | 编码器选择错误 | 确保预设使用 `h264_nvenc` 或 `hevc_nvenc`，而非软件编码器 |

## 使用绑定挂载时如何释放磁盘空间

::: danger 在使用宝塔、cPanel 或其他宿主机文件管理器清理录制文件**之前**，请先阅读本节。
在宿主机上直接对已经作为 Docker 绑定挂载（bind mount）传入 rust-srec 容器的目录进行**清理操作**，可能导致容器直到重启前都无法写入录制文件——哪怕宿主机显示有空闲空间。这是 Linux VFS 的行为，不是 rust-srec 的 bug。一旦知道原因，绕过方法非常简单。
:::

### 安全的清理方式（推荐）

按优先级排序：

1. **通过 rust-srec 网页界面删除录制文件**。应用会从容器内以自己的文件系统视图删除文件——永远安全、永远正确。
2. **`docker exec -it <容器> rm -rf /rec/<路径>`**——在容器自己的挂载命名空间里操作，永远安全。
3. **`docker exec -it <容器> sh`** 进容器交互式清理。
4. **扩容宿主机底层卷**（任何方式：云盘扩容、LVM extend、加盘）。永远安全。

### 危险的清理方式（会导致"清理磁盘后录制不恢复"）

这些都是触发 [#508](https://github.com/hua0512/rust-srec/issues/508) 及类似问题的典型场景：

1. 宿主机上执行 **`mv /host/rec /host/rec_backup`** 或任何对挂载源目录的重命名。
2. 宿主机上 **`rm -rf /host/rec`** 然后 **`mkdir /host/rec`** 重建目录。新建的目录是一个新的 inode，原有的绑定挂载**并不指向**它。
3. 用**宝塔面板文件管理器**对作为 Docker 绑定挂载源的目录使用默认删除。宝塔的"删除"实际上是"移动到回收站"，等价于上面的第 1 种模式。
4. 任何通过"移动到回收站/备份目录"实现"安全删除"的 GUI 或命令行工具。

### 原因简述

Docker 绑定挂载绑定的是 **inode**，不是路径。当您在宿主机上重命名或重建目录时，容器内 `/rec` 挂载指向的 inode 保持不变——但绝大多数 Linux 文件系统（ext4、xfs）在 `nlink == 0`（也就是"已删除但仍被引用"）的目录下拒绝创建新条目。于是 `create_dir_all` 永远返回 `ENOENT`，哪怕磁盘有大量空闲空间。只有销毁并重建容器的挂载命名空间（`docker restart`）才能恢复。

### rust-srec 如何检测和上报

**输出根写入门（output-root write gate）**会在一次监视周期内捕获这类故障，并：

- 把 `/health` 里的 `output-root` 组件翻转为 `Degraded`，标注 `error_kind: not_found` 和受影响挂载点的路径。
- 通过每个已启用的通知渠道（Discord、Email、Telegram、Gotify、Webhook、Web Push）发出**恰好一条** critical 级的 `output_path_inaccessible` 通知。文案会根据错误类型分支——`not_found` 变体包含"请重启容器"的恢复说明，设置 `RUST_SREC_LOCALE=zh-CN` 后显示中文。
- 在文件系统边界上短路后续的下载尝试，防止日志被级联重试淹没、也防止数据库 outbox 被大量写入。
- 将每个受影响的主播状态切换为 `OUT_OF_SPACE`，在主播列表中可直接看到。

执行 `docker restart` 后，容器启动时写入门会通过一次有界 5 秒的启动探测重新初始化。如果宿主机路径已修复，写入门会回到 `Healthy`，录制在下一个监视周期恢复。

### 如果实际原因是磁盘真的写满（不是挂载失效）

如果 `/health` 中的 `output-root` 显示 `Degraded`、`error_kind: storage_full`，**不需要重启**。通过上面任一安全清理方式释放空间即可，写入门会在下一次尝试下载时（约 30 秒内）自动恢复——恢复钩子会清除所有受影响主播的退避状态，整个队列在同一次监视周期内恢复运行。

### 多挂载部署：`RUST_SREC_OUTPUT_ROOTS`

启动探测会自动发现**所有配置层级**的挂载根——**全局**、**平台**、**模板**以及**单主播**的 `output_folder` 设置。它使用运行时相同的缓存路径并行合并每个主播的有效配置，去重解析后的根路径，并在容器启动时并行探测。对于典型的单挂载布局（如 `/rec`）这已经开箱即用；对于异构布局，任何主播实际可能写入的挂载都会在第一秒就被预检。

您只有在以下情况需要显式设置 `RUST_SREC_OUTPUT_ROOTS`：

- 希望探测尚未被任何主播配置引用的挂载（例如计划迁移的新卷），或
- 希望覆盖默认的解析启发式——例如在 `/rec/{platform}/...` 布局上强制使用单一 `/rec` 门键，将所有平台合并到同一个门条目下。

```env
RUST_SREC_OUTPUT_ROOTS=/rec,/mnt/backup-slow
```

值为逗号分隔的绝对路径列表。详见[配置说明](./configuration.md#后端服务)。

## 访问应用

- **Web 界面**：`http://localhost:[FRONTEND_PORT]` (默认：http://localhost:15275)
- **API 参考 (Swagger)**：`http://localhost:[API_PORT]/api/docs` (默认：http://localhost:12555/api/docs)

## 更新版本

更新到最新版本：
```bash
docker-compose pull
docker-compose up -d
```
