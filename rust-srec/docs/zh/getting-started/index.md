# 介绍

**rust-srec** 是一个基于 Rust 构建的自托管自动直播录制器。它支持 14 个平台，可通过 Web 界面和 REST API 完成录制、弹幕采集、后处理和任务管理。

## 核心功能

- **多平台支持**：支持 Bilibili、抖音、斗鱼、虎牙、Twitch、TikTok 等平台
- **自动录制**：主播开播时自动开始录制
- **弹幕采集**：同步录制弹幕
- **DAG 管道**：自定义后处理工作流
- **4 层配置**：从全局默认到主播专属的精细化配置
- **REST API**：完整的 API 及 Swagger 文档
- **JWT 认证**：安全的 JWT 令牌认证
- **Docker 支持**：便捷的 Docker 部署

## 快速链接

- [完成第一次录制](./first-recording.md)
- [安装指南](./installation.md)
- [Docker 部署](./docker.md)
- [配置说明](./configuration.md)

## 系统要求

- **Rust**：1.95 或更高版本（仅从源码构建时需要）
- **数据库**：SQLite（内置）
- **操作系统**：Linux、macOS、Windows
