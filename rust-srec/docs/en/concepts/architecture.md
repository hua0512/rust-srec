# System overview {#system-architecture}

Rust-Srec monitors configured channels, records live video and chat, and runs post-processing workflows. The web interface and API control the same backend.

```mermaid
flowchart LR
    UI[Web interface or API] --> B[Backend]
    P[Streaming platforms] --> B
    B --> DB[(SQLite: configuration and history)]
    B --> FILES[Recordings and chat files]
    FILES --> W[Post-processing workflows]
    B --> N[Notifications]
```

## Recording lifecycle

1. The backend checks a streamer's live status using its effective settings and schedule.
2. When recording is allowed and capacity is available, a download engine writes video files. Enabled danmu collection writes the matching chat files.
3. Size or duration limits can split a recording into segments. Segment workflows process completed files; paired workflows wait for video and chat from the same segment.
4. When the session ends and earlier processing finishes, its session-complete workflow runs.
5. Notifications report subscribed events. Sessions and outputs remain available in the interface.

## Deployment and storage

The standard installation uses one backend and one SQLite database. Docker packages the backend and frontend separately; desktop runs its recording services locally. Recorded files live on the filesystem, while the database stores configuration, sessions, jobs, and file records.

Back up both the database and files. A configuration export is not a recording backup, and deleting a database record does not necessarily delete the file. See [Backup and restore](../operations/backup-restore.md) and [deletion behavior](../operations/data-governance.md#deletion-semantics).

## Configure and operate the recorder

- [Configuration layers](./configuration.md): defaults, templates, and overrides.
- [Recording engines](./engines.md): compatibility and engine options.
- [Workflows](./pipeline.md): processing triggers and file routing.
- [Monitoring](../operations/monitoring.md): health, logs, and recovery actions.

For service boundaries, event delivery, database transactions, and Rust interfaces, see [Runtime architecture](../development/architecture.md).

<div id="high-level-topology" class="legacy-section">

This section is now in [Runtime architecture](../development/architecture.md#high-level-topology).

</div>

<div id="runtime-root-servicecontainer" class="legacy-section">

This section is now in [Runtime architecture](../development/architecture.md#runtime-root-servicecontainer).

</div>

<div id="service-container-responsibilities" class="legacy-section">

This section is now in [Runtime architecture](../development/architecture.md#service-container-responsibilities).

</div>

<div id="core-components-what-each-one-actually-does" class="legacy-section">

This section is now in [Runtime architecture](../development/architecture.md#core-components-what-each-one-actually-does).

</div>

<div id="runtimecoordinator-recording-startup-and-cancellation" class="legacy-section">

This section is now in [Runtime architecture](../development/architecture.md#runtimecoordinator-recording-startup-and-cancellation).

</div>

<div id="configservice-configuration-hot-reload" class="legacy-section">

This section is now in [Runtime architecture](../development/architecture.md#configservice-configuration-hot-reload).

</div>

<div id="streamermanager-committed-metadata-snapshots" class="legacy-section">

This section is now in [Runtime architecture](../development/architecture.md#streamermanager-committed-metadata-snapshots).

</div>

<div id="scheduler-actor-model-orchestration" class="legacy-section">

This section is now in [Runtime architecture](../development/architecture.md#scheduler-actor-model-orchestration).

</div>

<div id="streammonitor-detect-filter-outbox" class="legacy-section">

This section is now in [Runtime architecture](../development/architecture.md#streammonitor-detect-filter-outbox).

</div>

<div id="sessionlifecycle-single-owner-of-session-state" class="legacy-section">

This section is now in [Runtime architecture](../development/architecture.md#sessionlifecycle-single-owner-of-session-state).

</div>

<div id="downloadmanager-downloads-engine-abstraction" class="legacy-section">

This section is now in [Runtime architecture](../development/architecture.md#downloadmanager-downloads-engine-abstraction).

</div>

<div id="danmuservice-chat-capture" class="legacy-section">

This section is now in [Runtime architecture](../development/architecture.md#danmuservice-chat-capture).

</div>

<div id="pipelinemanager-job-queue-dag-worker-pools" class="legacy-section">

This section is now in [Runtime architecture](../development/architecture.md#pipelinemanager-job-queue-dag-worker-pools).

</div>

<div id="notificationservice-event-fan-out" class="legacy-section">

This section is now in [Runtime architecture](../development/architecture.md#notificationservice-event-fan-out).

</div>

<div id="repository-row-writes" class="legacy-section">

This section is now in [Runtime architecture](../development/architecture.md#repository-row-writes).

</div>

<div id="downloader-rust-interfaces" class="legacy-section">

This section is now in [Runtime architecture](../development/architecture.md#downloader-rust-interfaces).

</div>

<div id="ffmpeg-recording-events" class="legacy-section">

This section is now in [Runtime architecture](../development/architecture.md#ffmpeg-recording-events).

</div>

<div id="key-flows" class="legacy-section">

This section is now in [Runtime architecture](../development/architecture.md#key-flows).

</div>

<div id="recording-lifecycle-end-to-end" class="legacy-section">

This section is now in [Runtime architecture](../development/architecture.md#recording-lifecycle-end-to-end).

</div>

<div id="api-request-flow-control-plane" class="legacy-section">

This section is now in [Runtime architecture](../development/architecture.md#api-request-flow-control-plane).

</div>

<div id="scheduler-state-and-backoff" class="legacy-section">

This section is now in [Runtime architecture](../development/architecture.md#scheduler-state-and-backoff).

</div>

<div id="reliable-lifecycle-feedback" class="legacy-section">

This section is now in [Runtime architecture](../development/architecture.md#reliable-lifecycle-feedback).

</div>

<div id="event-driven-communication" class="legacy-section">

This section is now in [Runtime architecture](../development/architecture.md#event-driven-communication).

</div>

<div id="output-root-write-gate" class="legacy-section">

This section is now in [Runtime architecture](../development/architecture.md#output-root-write-gate).

</div>

<div id="service-ownership" class="legacy-section">

This section is now in [Runtime architecture](../development/architecture.md#service-ownership).

</div>

<div id="observability-health-and-shutdown" class="legacy-section">

This section is now in [Runtime architecture](../development/architecture.md#observability-health-and-shutdown).

</div>

<div id="backend-rust-interfaces" class="legacy-section">

This section is now in [Runtime architecture](../development/architecture.md#backend-rust-interfaces).

</div>

<div id="actor-terminal-and-wake-policies" class="legacy-section">

This section is now in [Runtime architecture](../development/architecture.md#actor-terminal-and-wake-policies).

</div>
