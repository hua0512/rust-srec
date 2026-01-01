# Introduction

**Rust-Srec** is an automatic online streaming recorder built with Rust. It supports 12 platforms and provides a comprehensive set of features for recording, processing, and managing live streams.

## Key Features

- **Multi-Platform Support**: Record from Bilibili, Douyin, Douyu, Huya, Twitch, TikTok, and more
- **Automatic Recording**: Automatically start recording when streamers go live
- **Danmaku Collection**: Capture live chat/danmaku alongside video
- **DAG Pipeline**: Post-processing with customizable directed acyclic graph workflows
- **4-Layer Configuration**: Fine-grained control from global defaults to per-streamer overrides
- **REST API**: Full-featured API with Swagger documentation
- **JWT Authentication**: Secure access with JWT tokens
- **Docker Support**: Easy deployment with Docker and docker-compose

## Quick Links

- [Installation Guide](./installation.md)
- [Docker Deployment](./docker.md)
- [Configuration](./configuration.md)

## System Requirements

- **Rust**: 2024 edition (for building from source)
- **Database**: SQLite (bundled)
- **OS**: Linux, macOS, Windows
