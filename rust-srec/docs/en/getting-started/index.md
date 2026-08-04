# Introduction

**Rust-Srec** is a self-hosted automatic livestream recorder built with Rust. It supports 14 platforms and provides recording, chat capture, post-processing, and task management from a web interface and REST API.

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

- [Make Your First Recording](./first-recording.md)
- [Installation Guide](./installation.md)
- [Docker Deployment](./docker.md)
- [Configuration](./configuration.md)

## System Requirements

- **Rust**: 1.95 or newer (only when building from source)
- **Database**: SQLite (bundled)
- **OS**: Linux, macOS, Windows
