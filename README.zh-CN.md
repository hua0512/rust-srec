<p align="center">
  <img src="rust-srec/docs/public/stream-rec-orange.svg" width="120" alt="rust-srec logo" />
</p>

<h1 align="center">rust-srec</h1>

<p align="center">
  <b>全自动直播录制工具，主播开播即开录。</b><br/>
  配好一次，长期省心，你常看的主流直播平台都能覆盖。
</p>

<p align="center">
  <a href="./README.md">English</a> ·
  <a href="https://docs.srec.rs/zh/">文档</a> ·
  <a href="https://docs.srec.rs/zh/getting-started/">快速上手</a> ·
  <a href="https://docs.srec.rs/zh/getting-started/docker">Docker</a> ·
  <a href="https://docs.srec.rs/zh/release-notes/">更新日志</a>
</p>

---

## 简介

rust-srec 是一款基于 Rust 编写的直播录制工具，主打自动化和稳定运行。添加主播后无需盯守，一旦开播就会自动开录；平台支持时，弹幕也会与视频一起保存。内置 FLV / HLS 修复能力，可以处理时间戳错乱、元数据缺失等常见问题。

部署轻量，Docker 一行命令即可启动，自带 Web 管理界面与 REST API。

## 支持平台

Bilibili、抖音、斗鱼、虎牙、Twitch、TikTok、AcFun、Picarto、小红书、TwitCasting、微博、PandaTV（已停服）

## 快速上手（Docker）

最推荐的部署方式 —— Linux 与 macOS：

```bash
curl -fsSL https://docs.srec.rs/install.sh | SREC_LANG=zh bash
```

Windows PowerShell：

```powershell
$env:SREC_LANG = "zh"; irm https://docs.srec.rs/install.ps1 | iex
```

安装脚本会下载 compose 与环境变量文件、生成所需密钥、检测 NVIDIA 支持，并询问是否直接启动服务。启动后打开 Web 界面，按照[快速上手指南](https://docs.srec.rs/zh/getting-started/)继续配置即可。

若不想跟随 `latest`，可用 `VERSION` 指定版本；镜像 tag 以 `v` 开头：

```bash
curl -fsSL https://docs.srec.rs/install.sh | SREC_LANG=zh VERSION=v0.5.1 bash
```

> 将远程脚本直接管道给 shell，执行的是服务器当次返回的内容。如需先行审阅，可下载 [install.sh](https://docs.srec.rs/install.sh) 检查后再运行本地副本，或参考 [Docker 部署文档](https://docs.srec.rs/zh/getting-started/docker)中的手动步骤。

想直接下载二进制或自行编译，请参考[安装指南](https://docs.srec.rs/zh/getting-started/installation)。

## 核心功能

- **多平台支持**：开箱即用，覆盖 12 个主流直播平台。
- **三种下载引擎**：可选 `ffmpeg`、`streamlink`，以及内置的 Rust 引擎 `mesio`。
- **后处理流水线**：自动转码、切片，或将录像交给你自己的脚本继续处理。
- **Web 界面与 REST API**：浏览器里管理主播，API 用于自动化（JWT 鉴权，OpenAPI 文档位于 `/api/docs`）。
- **稳定可靠**：SQLite 存储状态，启动时自动完成数据库迁移。

## 命令行工具

仓库内还附带两个独立的 CLI：

| 工具 | 用途 | 文档 |
| --- | --- | --- |
| `strev` | 解析直播链接，提取支持平台的流媒体信息。 | [`strev-cli/README.md`](./strev-cli/README.md) |
| `mesio` | 命令行下载并修复 FLV / HLS 流。 | [`mesio-cli/README.md`](./mesio-cli/README.md) |

## 相关文档

- [配置参考](https://docs.srec.rs/zh/getting-started/configuration)
- [架构总览](https://docs.srec.rs/zh/concepts/architecture)
- [录制引擎说明](https://docs.srec.rs/zh/concepts/engines)
- [常见问题](https://docs.srec.rs/zh/getting-started/faq)

## 参与开发

本仓库是一个 Cargo workspace，主要目录如下：

- `rust-srec/`：录制器后端（API、调度器、流水线、数据库）
- `strev-cli/`、`mesio-cli/`：上面提到的两个命令行工具
- `crates/`：可复用的库 —— 平台解析、FLV / HLS / TS 协议、`mesio` 下载引擎以及流修复工具

常用命令：

```bash
cargo build                       # 编译全部
cargo test                        # 运行测试
cargo clippy -- -D warnings       # 静态检查
cargo fmt                         # 格式化代码
```

欢迎提交 PR，详见 [CONTRIBUTING.md](./CONTRIBUTING.md)。

## 开源协议

基于 [MIT 协议](./LICENSE)发布。
