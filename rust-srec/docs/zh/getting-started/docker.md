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

## 访问应用

- **Web 界面**：`http://localhost:[FRONTEND_PORT]` (默认：http://localhost:15275)
- **API 参考 (Swagger)**：`http://localhost:[API_PORT]/api/docs` (默认：http://localhost:12555/api/docs)

## 更新版本

更新到最新版本：
```bash
docker-compose pull
docker-compose up -d
```
