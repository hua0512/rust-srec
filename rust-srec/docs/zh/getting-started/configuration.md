# 基础配置 {#配置}

安装完成并[成功录制一次](./first-recording.md)后，先设置以下默认值，再添加更多主播。

## 选择录制目录

打开**设置 → 全局 → 输出目录**，选择可写的录制目录。Docker 在容器内使用 `/app/output`，Compose 将它映射到宿主机的 `OUTPUT_DIR`。全新 systemd 安装使用 `/var/lib/rust-srec/output`；已有安装保留数据库中保存的目录。

在输出目录布局中使用 `{streamer}/%Y-%m-%d`，可按主播和日期分类；文件名模板可使用 `%H-%M-%S_{title}`。详见[文件名占位符](../reference/filenames.md)及[存储路径](../operations/storage.md)。

## 选择引擎和录制上限

首次录制可保留 Mesio，除非平台要求其他引擎。兼容性和功能对比见[录制引擎](../concepts/engines.md)。

将下载并发设为网络和磁盘能够持续承受的数量。可通过时长或分段大小限制拆分长录制。默认值和单位见[设置参考](../reference/settings.md)。

## 按需配置平台凭据

按对应[平台指南](../platforms/)确认是否需要登录或 Cookie。多个主播共用账号时，在平台层配置凭据；需要不同账号时，使用模板或主播覆盖。

## 使用模板复用设置

为共用的录制设置创建模板，并分配给相关主播。主播覆盖优先于模板。继承规则和修改生效时间见[配置层级](../concepts/configuration.md)。

## 可选录制功能

- 启用[弹幕录制与统计](../guides/danmu.md)，保存直播聊天。
- 配置[录制时间安排](../guides/schedules.md)，限制录制时段。
- 创建[工作流](../concepts/pipeline.md)，执行转换、缩略图或上传。
- 配置[通知](../concepts/notifications.md)，接收录制失败或存储告警。

修改设置后，再验证一次录制，并确认文件保存在预期目录。

<div id="基础配置" class="legacy-section">

此节内容已移至[完成第一次录制](./first-recording.md).

</div>

<div id="添加第一个主播" class="legacy-section">

此节内容已移至[完成第一次录制](./first-recording.md).

</div>

<div id="全局设置" class="legacy-section">

此节内容已移至[设置参考](../reference/settings.md#全局设置).

</div>

<div id="文件配置-file-configuration" class="legacy-section">

此节内容已移至[设置参考](../reference/settings.md#文件配置-file-configuration).

</div>

<div id="弹幕统计" class="legacy-section">

此节内容已移至[设置参考](../reference/settings.md#弹幕统计).

</div>

<div id="资源限制-resource-limits" class="legacy-section">

此节内容已移至[设置参考](../reference/settings.md#资源限制-resource-limits).

</div>

<div id="并发与性能-concurrency-performance" class="legacy-section">

此节内容已移至[设置参考](../reference/settings.md#并发与性能-concurrency-performance).

</div>

<div id="网络与系统-network-system" class="legacy-section">

此节内容已移至[设置参考](../reference/settings.md#网络与系统-network-system).

</div>

<div id="保留策略-retention" class="legacy-section">

此节内容已移至[设置参考](../reference/settings.md#保留策略-retention).

</div>

<div id="流水线配置-pipeline-configuration" class="legacy-section">

此节内容已移至[设置参考](../reference/settings.md#流水线配置-pipeline-configuration).

</div>

<div id="环境变量" class="legacy-section">

此节内容已移至[环境变量](../reference/environment.md#环境变量).

</div>

<div id="通用" class="legacy-section">

此节内容已移至[环境变量](../reference/environment.md#通用).

</div>

<div id="路径" class="legacy-section">

此节内容已移至[环境变量](../reference/environment.md#路径).

</div>

<div id="关闭" class="legacy-section">

此节内容已移至[环境变量](../reference/environment.md#关闭).

</div>

<div id="网络" class="legacy-section">

此节内容已移至[环境变量](../reference/environment.md#网络).

</div>

<div id="安全与认证" class="legacy-section">

此节内容已移至[环境变量](../reference/environment.md#安全与认证).

</div>

<div id="登录限流" class="legacy-section">

此节内容已移至[环境变量](../reference/environment.md#登录限流).

</div>

<div id="令牌过期" class="legacy-section">

此节内容已移至[环境变量](../reference/environment.md#令牌过期).

</div>

<div id="浏览器通知-web-push-vapid" class="legacy-section">

此节内容已移至[环境变量](../reference/environment.md#浏览器通知-web-push-vapid).

</div>

<div id="后端服务" class="legacy-section">

此节内容已移至[环境变量](../reference/environment.md#后端服务).

</div>

<div id="资源限制-docker" class="legacy-section">

此节内容已移至[环境变量](../reference/environment.md#资源限制-docker).

</div>

<div id="文件名模板变量" class="legacy-section">

此节内容已移至[文件名模板变量](../reference/filenames.md#文件名模板变量).

</div>

<div id="大括号变量-curly-brace-variables" class="legacy-section">

此节内容已移至[文件名模板变量](../reference/filenames.md#大括号变量-curly-brace-variables).

</div>

<div id="百分号占位符-percent-placeholders-ffmpeg-风格" class="legacy-section">

此节内容已移至[文件名模板变量](../reference/filenames.md#百分号占位符-percent-placeholders-ffmpeg-风格).

</div>

<div id="流水线目标路径占位符" class="legacy-section">

此节内容已移至[文件名模板变量](../reference/filenames.md#流水线目标路径占位符).

</div>
