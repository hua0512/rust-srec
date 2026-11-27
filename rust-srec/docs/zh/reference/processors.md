# 处理器参考

编辑工作流或任务预设时选择处理器。完整示例见[创建工作流](../concepts/pipeline.md)。

## 内置处理器 (Processors) {#内置处理器-processors}

每个管道步骤由一个专门的处理器执行：

| 处理器 ID | 功能 | 核心参数 |
|----------|------|---------|
| `remux` | 更改容器格式，也可选择重新编码 | `format`, `video_codec`, `audio_codec`, `crf`, `preset`, `hwaccel`, `start_time`, `duration`, `end_time`, `include_recording_metadata`, `remove_input_on_success` |
| `danmaku_factory` | 使用 DanmakuFactory 将弹幕 XML 转换为 ASS 字幕 | `binary_path`, `args`, `extra_args`, `delete_source_xml_on_success` |
| `ass_burnin` | 将 ASS 字幕硬烧录进对应的视频 | `match_strategy`, `video_codec`, `crf`, `preset`, `fonts_dir`, `delete_source_videos_on_success` |
| `thumbnail` | 从视频中提取画面作为图片 | `timestamp_secs`, `width`, `quality`, `preserve_resolution` |
| `audio_extract` | 提取音轨 | `format`, `bitrate`, `sample_rate`, `channels`, `output_path` |
| `compression` | 将文件打包为 ZIP 或 tar.gz 归档 | `format`, `compression_level`, `output_path`, `overwrite`, `preserve_paths` |
| `rclone` | 云端同步 | `destination_root`, `config_path`, `operation`, `time_anchor`, `max_retries`, `bwlimit`, `bwlimit_file`, `transfers`, `checkers`, `tpslimit`, `tpslimit_burst`, `multi_thread_streams`, `multi_thread_cutoff`, `args`, `proxy_route` |
| `rship` | 通过 [rship](https://hua0512.github.io/rship/) 进行支持崩溃续传的多云传输 | `destination_root`, `binary_path`, `config_path`, `state_dir`, `operation`, `time_anchor`, `max_retries`, `bwlimit`, `transfers`, `adaptive`, `part_concurrency`, `chunk_size`, `max_buffer_mem`, `tpslimit`, `drive_stop_on_upload_limit`, `checksum`, `strict_source`, `args`, `proxy_route` |
| `baidupcs` | 通过 BaiduPCS-Go 上传到百度网盘 | `destination_root`, `binary_path`, `config_dir`, `policy`, `norapid`, `max_retries`, `remove_source_after_upload`, `time_anchor`, `args`, `proxy_route` |
| `copy_move` | 复制或移动本地文件 | `operation`, `destination`, `time_anchor`, `overwrite`, `exclude_patterns` |
| `metadata` | 使用 FFmpeg 将元数据标签写入媒体文件 | `title`, `artist`, `date`, `album`, `comment`, `custom`, `remove_input_on_success` |
| `delete` | 删除它收到的文件，即其所依赖步骤的输出 | `max_retries`, `retry_delay_ms` |
| `execute` | 运行程序或自定义 Shell 命令 | `program`, `args` 或 `command`, `scan_output_dir`, `scan_extension` |

基于 FFmpeg 的处理器在设置了 `FFMPEG_PATH` 时使用该路径，调用外部工具的处理器读取各自的[路径变量](./environment.md#外部工具)。

当工作流步骤未单独配置自动重试时，`rclone`、`rship` 和 `baidupcs` 会在同一次任务运行中重试失败的上传；每次重试都会记入任务日志，任务进度和顶栏会显示当前是第几次尝试。当工作流步骤将 `retry.max_attempts` 设为大于 `1` 时，每次任务运行仅尝试一次，将重试交给工作流步骤的退避计划，以便在较长等待期间释放上传名额。`rclone` 和 `rship` 遇到限流或配额不足时也会立即停止本次运行内的剩余重试；遇到配置错误、凭据失效、同步冲突或路径不存在等永久性错误时会直接失败而不再重试。

### 上传代理 {#upload-proxy}

每个 `rclone`、`rship` 和 `baidupcs` 步骤都可以在 **代理**（`proxy_route`）中选择连接方式：**继承**（默认）跟随全局 **默认代理**，也可以为该步骤选择 **直连**、**系统代理** 或某个[已保存的代理](../concepts/configuration.md#saved-proxies)。上传的目标是云存储而不是平台，因此平台、模板和主播的代理设置不适用于上传。这些工具从环境变量读取代理：**直连** 时启动它们不带任何代理变量，**系统代理** 时沿用服务器的变量，已保存的代理则以 `HTTP_PROXY` 和 `HTTPS_PROXY`（对 `rship` 同时设置 `RSHIP_PROXY`，含登录信息）传入。`http`、`https`、`socks5` 和 `socks5h` 代理均可使用。连接方式在任务开始时确定；所选的已保存代理被删除后，任务会失败而不会改为直连。仍被任务预设或流水线步骤选用的已保存代理不能删除，其密码在任务日志和工具输出中会被隐藏。BaiduPCS-Go 预设账号卡片上的登录和容量查询跟随默认代理，上传过程中的自动重新登录使用该步骤的代理；退出登录等只读取本地账号的命令不使用代理。

选择 **系统代理** 以外的方式时，工具自身的代理设置不能覆盖它。rclone 会忽略环境变量 `RCLONE_HTTP_PROXY`，`rship` 也会忽略宿主机环境变量 `RSHIP_PROXY`；**额外参数** 中含有 `--http-proxy`（rclone）或 `--proxy`（rship）的步骤会失败。对 rclone 和 `rship` 而言，步骤的代理只用于通过 HTTP 连接的远程（例如大多数云存储）；SFTP、FTP 等不使用 HTTP 的远程以及本机上的远程不经由该代理（对 rclone 而言，这类远程使用 rclone 配置文件中该远程自己的代理选项）。BaiduPCS-Go 会优先使用其自身配置中设置的代理（`config set -proxy`），因此在清除该设置之前，步骤、登录或容量查询都会失败；账号卡片仍会显示已登录的账号以及无法读取容量的原因。

### 执行程序（`execute`） {#execute-execute}

在预设编辑器或工作流步骤的配置对话框中，通过**执行模式**选择 **Shell 命令**或**程序和参数**。已有 Shell 命令会保持原样，以 Shell 模式打开。切换模式会清空原有命令或程序及参数；输出扫描设置由两种模式共用并保留。

在程序模式下，在**程序**中填写可执行文件，按顺序使用**添加参数**逐项输入。每个文本框代表一个完整参数，可包含空格、引号、换行或空字符串。删除文本框才会省略该参数；留空会传入空参数。保存时只包含所选模式的字段。

使用 `program` 和 `args` 可以直接运行可执行程序，无需经过 Shell：

```json
{
  "program": "ffmpeg",
  "args": ["-nostdin", "-n", "-i", "{input}", "-c", "copy", "/recordings/converted/{streamer}_%Y%m%d_%H%M%S.mp4"],
  "scan_output_dir": "/recordings/converted",
  "scan_extension": "mp4"
}
```

工作流步骤不会收到输出路径，因此 `{output}`、`{outputN}` 和 `{outputs_json}` 在工作流中为空。请在参数里指定文件名，并设置 `scan_output_dir`（支持相同的占位符），新文件就会传递给下一步：命令开始前目录中不存在、且在命令开始后修改过的文件即为该步骤的输出。未设置扫描目录时，输入会原样传递。该步骤的文件大小指标和成功输入列表覆盖它收到的所有输入及检测到的所有输出；扫描目录无法读取时，步骤会失败，而不是报告没有输出。

`program` 是 `PATH` 中的固定可执行程序名或可执行文件路径，不展开占位符。`args` 中每一项都是一个参数，空字符串也会保留。参数支持与 `command` 相同的文件、元数据、JSON 数组和时间占位符。在配对分段和会话完成管道中，`{manifest_json}` 展开为会话配对的 JSON（按分段记录每个视频对应的弹幕文件）；其他管道中展开为 `null`。插入的取值保持原样：引号、空格、Shell 运算符、环境变量引用及取值内的其他占位符都不会被解释。不要在参数外额外添加 Shell 引号。被调用的程序仍会按自身规则解释选项。

省略 `args` 表示不传入参数。`program`（可带 `args`）与 `command` 只能选择一种，混用会使步骤失败。在 Windows 上，`program` 拒绝 `.bat` 和 `.cmd` 文件，因为 Windows 会隐式通过 Shell 运行它们；批处理脚本请使用 `command`。两种模式均保留输出目录扫描、管道输出处理、超时和进程清理行为。

Shell 语法取决于服务器的操作系统，与浏览器的操作系统无关。例如，`ffmpeg -nostdin -n -i {input} -c copy {input}.mp4` 使用固定原生可执行程序及占位符参数；服务器已安装 FFmpeg 时，该示例符合两个平台所支持的模板语法。

不含已识别占位符的命令模板保留原有 Shell 行为。含占位符时，编译器只接受明确限定的语法，通过进程局部绑定传递替换数据。占位符可用于普通参数单词或文件重定向目标，命令名必须固定。未知和越界占位符保持字面文本。未加引号的空值不产生参数，已有引号会保留空参数；空重定向路径在启动前报错。

| 含占位符的模板 | 支持的结构 |
| --- | --- |
| POSIX | 未引用、单引号和双引号中的参数片段；`;`、LF 换行、`&&`、`||`、管道、`<`、`>`、`>>`，以及 `2>&1` 等固定文件描述符复制 |
| Windows | 固定的原生可执行程序名或路径；带引号或不带引号的参数片段；`&`、`&&`、`||`、文件重定向及固定文件描述符复制。原生命令前后可附加不含占位符的 `echo`、`ver` 阶段。 |

含占位符的模板会在启动前拒绝命令替换、算术、参数展开、here-document、命令分组、前置变量赋值、命令名展开（包括 POSIX 花括号形式）和嵌套 Shell 包装命令。

Windows 还拒绝管道、多行模板、批处理脚本、含占位符的内置命令阶段、模板中的字面 `%`/`!`/`^` 字符、替换单词中的控制字符，以及含义不明确的字面引号和反斜杠组合。这些模板字符限制也适用于引号内；请把这类数据放入 `args` 或占位符取值。

Windows 先组装完整参数，再按常见 C 风格及 Shell32 解析器的反斜杠引号规则编码。使用自定义参数解析器的程序必须接受这种编码；请遵循被调用程序的参数或数据接口要求。重定向文件名使用独立规则，不能含双引号。

Windows 上含占位符的命令要求使用常规原生可执行文件路径（不支持通配符程序名和 `\\?\` 扩展路径），且总长度不得超过 Windows `cmd.exe` 的 8,191 字符限制。遇到 `{inputs_json}` 或 `{manifest_json}` 等大型 JSON 数组时，请改用 `program` 和 `args`（或通过文件传递数据）。

不支持的形式请改用 `program` 与 `args`，或使用具有明确参数/数据接口的固定脚本。不要把不可信取值放进被调用程序的代码或表达式参数中；程序仍会按自身规则解释选项。

### 归档（`compression`） {#归档-compression}

`compression` 处理器把输入文件打包成一个归档文件。它不会重新编码媒体，文件按原样放入归档。

| 配置项 | 说明 | 默认值 |
|--------|------|--------|
| `format` | `zip` 或 `targz`（gzip 压缩的 tar） | `zip` |
| `compression_level` | `0`–`9`。`0` 表示不压缩——`zip` 为仅存储条目，`targz` 为不压缩的 gzip 流——对已经压缩过的视频来说通常是更合适的选择。大于 `9` 的取值不会被收敛，而是让步骤直接失败。 | `6` |
| `output_path` | 要写入的归档路径。省略时根据第一个输入推导：同一目录、同一文件名，把输入的扩展名换成对应格式的扩展名，因此 `/rec/video.flv` 得到 `/rec/video.zip`。 | 自动推导 |
| `overwrite` | 覆盖该路径上已存在的归档。设为 `false` 时步骤会直接失败。 | `true` |
| `preserve_paths` | 在归档内按输入文件的完整路径存放（去掉开头的分隔符）：`/srv/recordings/x/a.mp4` 对应的条目是 `srv/recordings/x/a.mp4`。路径不会相对某个公共前缀截断。设为 `false` 时，所有条目都以文件名平铺在归档根目录。 | `false` |

该处理器支持批量输入，因此一个从依赖步骤收到多个文件的步骤会生成一个包含全部文件的归档。它的输出只有归档路径——输入文件不会被删除，而依赖它的 `delete` 步骤删除的是归档本身，而不是源文件。

两个输入若会存为同一个归档条目（通常是关闭 `preserve_paths` 时来自不同目录的同名文件），会在写入任何归档之前被拒绝。请开启 `preserve_paths` 或重命名其中一个。

ZIP 条目始终按 ZIP64 尺寸写入，因此单个超过 4 GiB 的录制文件也能正确打包。代价是每个条目多 40 字节，生成的归档仍可用常规 ZIP 工具打开。

### Rship（`rship`） {#rship-rship}

`rship` 处理器通过外部 [rship](https://hua0512.github.io/rship/) 命令行工具将录像传输到云存储或远程存储（Docker 镜像已内置；裸机部署需自行安装，若不在 `PATH` 中可通过 `RSHIP_PATH` 指定路径）。支持 S3 兼容存储（AWS S3、Cloudflare R2、Backblaze B2、MinIO、Wasabi 等）、Google Drive、Google Cloud Storage、OneDrive 与 SharePoint、Azure Blob Storage、Dropbox、WebDAV（Nextcloud、OwnCloud、Alist、Synology NAS、InfiniCloud）、SFTP、`crypt` 加密层及本地目录，具备可控缓冲内存上限（`--max-buffer-mem`）以及基于 SQLite WAL 的断点续传检查点。

- **远程配置与状态目录**：使用前需在 `rship` 的 `config.toml` 中配置远程（例如在容器内或宿主机执行 `rship auth`、`rship config add` 或 `rship config import-rclone`）。Docker 镜像默认设置 `RSHIP_CONFIG=/app/config/rship/config.toml` 与 `RSHIP_STATE_DIR=/app/config/rship/state`，将配置、OAuth 令牌和分片续传检查点统一保存在挂载的 `/app/config` 卷中；也可在预设中通过 `config_path` 和 `state_dir` 单独覆盖。`config.toml` 中的 `[defaults]` 与 `[profiles.<name>]` 预设同样作用于上传，可在 **额外参数** 中添加 `--profile <name>` 选用。
- **传输模式**：`copy`（默认）和 `sync` 会将本地输入路径原样传递给下游步骤；`move` 仅在远端写入确认后删除本地源文件，且不产生本地输出。`sync` 仅支持批量输入（两个及以上文件），会将这些文件更新为与本地副本一致；批次之外的远端文件不会被删除。
- **重试与并发上限**：每次尝试默认执行单轮传输（`--retries 1`，可通过 `args` 覆盖），已提交的分片由 `rship` 状态库在下次尝试时直接续传。遇到限流或每日上传配额耗尽（包括开启 **达到 Google Drive 每日上传上限时停止** 后触发的 750 GiB 每日上限）时，会立即停止本次运行内的剩余重试以释放上传名额，同时保留工作流步骤的后续计划重试资格。`rship` 与 `rclone`、`baidupcs` 共用 **最大上传数** 限制，并可在该上限内并发运行多个上传任务。

### 百度网盘（`baidupcs`） {#百度网盘-baidupcs}

`baidupcs` 处理器通过外部 [BaiduPCS-Go](https://github.com/qjfoidnh/BaiduPCS-Go) 命令行工具将录像上传到百度网盘（Docker 镜像已内置；裸机部署需自行安装，若不在 `PATH` 中可通过 `BAIDUPCS_PATH` 指定路径）。

- **登录**：在网页端打开任意 `baidupcs` 预设，通过账号卡片粘贴 Cookie 字符串（推荐）或 BDUSS + STOKEN 登录。凭据交给 BaiduPCS-Go 处理，登录会话保存在它的配置目录（`BAIDUPCS_GO_CONFIG_DIR`）中；卡片同时显示当前账号和网盘容量。勾选**记住凭据以便自动重新登录**后，凭据也会保存在服务器上（明文，与平台 Cookie 相同）：登录会话过期时，上传任务会在首次尝试前检查登录状态、并在重试前再补一次登录，无需人工干预。如果重放的登录被拒绝（通常是修改密码导致会话令牌失效），会发出高优先级的 `baidupcs_relogin_failed` 通知，并在一小时内暂停后续尝试——失效凭据只产生一条提醒，而不是每个任务都白白请求一次百度。退出登录会一并清除保存的凭据。
- **目标路径**：`destination_root` 支持 `{streamer}`、`{title}` 及时间占位符，始终解析为网盘的绝对路径。不存在的文件夹会在上传时自动创建。
- **重试**：BaiduPCS-Go 的退出码无法反映上传结果，rust-srec 会解析其逐文件输出来判定成败。无论是运行内重试、工作流重试还是手动重试任务，都会保留同一账号与目标目录下已确认上传成功的文件，仅重新上传尚未确认结果的文件；配合默认的 `skip` 策略和秒传检测，部分失败后的重试开销很小。批量上传中部分文件失败时，步骤会失败，并按文件分别记录结果（已上传、已跳过或失败）。由于所有文件都直接放在目标文件夹下，文件名相同的两个输入会在上传前被拒绝。被百度直接拒绝的文件（文件名不合法、超过大小限制、无法读取或容量已满）会记为失败，同一次运行内的后续尝试不会再重新上传。如果所有剩余失败文件都被永久拒绝，工作流步骤会直接失败，不再消耗后续重试次数（若同一批次同时包含永久拒绝和临时失败的文件，后续工作流重试仍会处理剩余失败文件）。大批量文件会拆成多次 BaiduPCS-Go 调用上传，使命令行长度不超过平台限制；上传前的登录检查不会写入任务日志。
- **限制**：单文件超过 128 GB 会被百度拒绝；传输中断后只能从头重传（BaiduPCS-Go v4 已不支持断点续传）。由于该工具的本地状态存储不支持并发写入，上传任务同一时间只运行一个 BaiduPCS-Go 进程；任务运行期间也请勿手动对同一配置目录执行 BaiduPCS-Go 命令。其余上传任务会排队等待当前上传完成，在顶栏中显示为排队中的上传，超时（步骤的 [`timeout_secs`](./workflows.md#按步骤设置重试与超时)，未设置时为 **IO 任务**超时）从任务开始运行时才计算。百度网盘上传与 rclone、rship 上传一起计入**最大上传数**，不计入**最大 IO 任务数**。发送文件前，BaiduPCS-Go 会读取整个文件以检查百度是否已有相同文件（开启**禁用秒传**时除外）；大型录制文件可能需要数分钟，期间任务显示**准备中…**。

- **登录注意事项**：凭据通过标准输入传递而非命令行参数，也不会写入 BaiduPCS-Go 的命令历史；登录成功后仅更新 `pcs_config.json` 中的当前账号条目。登录期间请勿对同一配置目录并发运行外部 `BaiduPCS-Go` 命令——如果配置文件在此期间被外部修改，更新会被拒绝。自定义程序必须兼容 BaiduPCS-Go v4 的交互式标准输入命令接口和 `pcs_config.json` 格式。登录超时时间为 60 秒，粘贴的凭据不能包含换行或 NUL 字符。
