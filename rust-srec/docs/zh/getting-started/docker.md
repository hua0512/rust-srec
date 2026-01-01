# Docker 部署

Docker 是运行 Rust-Srec 最简单且推荐的方式。

## 前置要求

- [Docker](https://docs.docker.com/get-docker/)
- [Docker Compose](https://docs.docker.com/compose/install/)

## 快速开始

1. 创建项目目录：
   ```bash
   mkdir rust-srec && cd rust-srec
   mkdir -p data config output logs
   ```

2. 下载示例配置文件：
   - [docker-compose.example.yml](/docker-compose.example.yml)
   - [.env.example](/.env.example) (英文) 或 [.env.zh.example](/.env.zh.example) (中文)

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

[`.env`](/.env.example) 文件包含了所有必要的环境变量。

| 变量 | 说明 |
|------|------|
| `JWT_SECRET` | JWT 签名密钥 (**必需**) |
| `SESSION_SECRET` | 前端会话加密密钥 (**必需**) |

::: tip 完整参考
有关所有可用环境变量及其说明的完整列表，请参阅 [环境变量参考](./configuration.md#environment-variables)。
:::

### docker-compose.yml

我们的标准示例 [docker-compose.example.yml](/docker-compose.example.yml) 包含了：
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

## 访问应用

- **Web 界面**：`http://localhost:[FRONTEND_PORT]` (默认：http://localhost:15275)
- **API 参考 (Swagger)**：`http://localhost:[API_PORT]/api/docs` (默认：http://localhost:12555/api/docs)

## 更新版本

更新到最新版本：
```bash
docker-compose pull
docker-compose up -d
```
