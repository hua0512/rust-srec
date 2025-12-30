# 介绍

**rust-srec** 是一个基于 Rust 构建的自动直播录制器。支持 12 个平台，提供全面的录制、处理和管理功能。

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

- [安装指南](./installation.md)
- [Docker 部署](./docker.md)
- [配置说明](./configuration.md)

## 系统要求

- **Rust**：2024 edition（源码编译）
- **数据库**：SQLite（内置）
- **操作系统**：Linux、macOS、Windows
