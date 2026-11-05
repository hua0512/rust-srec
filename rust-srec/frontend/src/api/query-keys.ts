/**
 * Query keys for the cached server reads, one factory per domain.
 *
 * Every key of a domain starts with that domain's `all`, and a record's own
 * reads (its filters, segments, logs) start with its `detail` key, so a prefix
 * invalidation refreshes lists and detail pages together. Parameter objects go
 * last: React Query hashes them by value, so equal parameters built in a route
 * loader and in the page share one entry.
 *
 * Account profiles, proxies and interrupted outputs keep their factories
 * beside their query options in `api/`.
 */

const streamers = {
  all: ['streamers'] as const,
  /** Every page of every streamer listing, whatever filters built it. */
  lists: () => [...streamers.all, 'list'] as const,
  list: (params: object) => [...streamers.lists(), params] as const,
  detail: (id: string) => [...streamers.all, 'detail', id] as const,
  filters: (id: string) => [...streamers.detail(id), 'filters'] as const,
  /** Every window of the streamer's check history. */
  checkHistories: (id: string) =>
    [...streamers.detail(id), 'check-history'] as const,
  checkHistory: (id: string, slots: number) =>
    [...streamers.checkHistories(id), slots] as const,
};

const sessions = {
  all: ['sessions'] as const,
  list: (params: object) => [...sessions.all, 'list', params] as const,
  detail: (id: string) => [...sessions.all, 'detail', id] as const,
  segments: (id: string) => [...sessions.detail(id), 'segments'] as const,
  danmuStatistics: (id: string) =>
    [...sessions.detail(id), 'danmu-statistics'] as const,
};

const pipelineAll = ['pipeline'] as const;

const jobs = {
  all: [...pipelineAll, 'jobs'] as const,
  detail: (id: string) => [...jobs.all, id] as const,
  progress: (id: string) => [...jobs.detail(id), 'progress'] as const,
  uploads: (id: string) => [...jobs.detail(id), 'uploads'] as const,
  logs: (id: string) => [...jobs.detail(id), 'logs'] as const,
  logTail: (id: string, offset: number | undefined) =>
    [...jobs.logs(id), 'tail', offset] as const,
};

const dags = {
  all: [...pipelineAll, 'dags'] as const,
  lists: () => [...dags.all, 'list'] as const,
  list: (params: object) => [...dags.lists(), params] as const,
  detail: (id: string) => [...dags.all, 'detail', id] as const,
};

const outputs = {
  all: [...pipelineAll, 'outputs'] as const,
  list: (params: object) => [...outputs.all, 'list', params] as const,
  summary: (search: string) => [...outputs.all, 'summary', search] as const,
  forSession: (sessionId: string) =>
    [...outputs.all, 'session', sessionId] as const,
};

const workflows = {
  all: [...pipelineAll, 'workflows'] as const,
  list: (params: object) => [...workflows.all, 'list', params] as const,
  infinite: (params: object) => [...workflows.all, 'infinite', params] as const,
  detail: (id: string) => [...workflows.all, 'detail', id] as const,
};

const pipeline = {
  all: pipelineAll,
  stats: () => [...pipelineAll, 'stats'] as const,
  stalledOperations: () => [...pipelineAll, 'stalled-operations'] as const,
  jobs,
  dags,
  outputs,
  workflows,
};

const jobPresets = {
  all: ['job-presets'] as const,
  list: (params: object) => [...jobPresets.all, 'list', params] as const,
  infinite: (params: object) =>
    [...jobPresets.all, 'infinite', params] as const,
  detail: (id: string) => [...jobPresets.all, 'detail', id] as const,
  byName: (name: string | null) =>
    [...jobPresets.all, 'by-name', name] as const,
};

const templates = {
  all: ['templates'] as const,
  list: () => [...templates.all, 'list'] as const,
  detail: (id: string) => [...templates.all, 'detail', id] as const,
};

const engines = {
  all: ['engines'] as const,
  list: () => [...engines.all, 'list'] as const,
  detail: (id: string) => [...engines.all, 'detail', id] as const,
  /**
   * Outside `all`: an engine write would otherwise re-probe every engine on
   * the page. The config in the key re-probes an engine whose settings changed.
   */
  test: (id: string, config: unknown) => ['engine-test', id, config] as const,
};

const config = {
  all: ['config'] as const,
  global: () => [...config.all, 'global'] as const,
};

const platformConfigs = {
  all: [...config.all, 'platforms'] as const,
  list: () => [...platformConfigs.all, 'list'] as const,
  detail: (id: string) => [...platformConfigs.all, 'detail', id] as const,
};

const notifications = {
  all: ['notifications'] as const,
  channels: () => [...notifications.all, 'channels'] as const,
  subscriptions: (channelId: string | undefined) =>
    [...notifications.all, 'subscriptions', channelId] as const,
  eventTypes: () => [...notifications.all, 'event-types'] as const,
  events: (query: object) => [...notifications.all, 'events', query] as const,
  browserCriticalEvents: () =>
    [...notifications.all, 'browser-critical-events'] as const,
  criticalDot: () => [...notifications.all, 'critical-dot'] as const,
};

const logging = {
  all: ['logging'] as const,
  config: () => [...logging.all, 'config'] as const,
  files: (from: string | undefined, to: string | undefined) =>
    [...logging.all, 'files', from, to] as const,
};

const baiduPcs = {
  all: ['baidupcs-status'] as const,
  status: (binaryPath: string, configDir: string) =>
    [...baiduPcs.all, binaryPath, configDir] as const,
};

export const queryKeys = {
  session: ['session'] as const,
  health: ['health'] as const,
  apiKeys: ['auth', 'api-keys'] as const,
  webPushPublicKey: ['web-push', 'public-key'] as const,
  platformDetection: (url: string) => ['platform-detection', url] as const,
  baiduPcs,
  streamers,
  sessions,
  pipeline,
  jobPresets,
  templates,
  engines,
  config,
  platformConfigs,
  notifications,
  logging,
};
