<script setup>
const items = [
  {
    title: '开发环境',
    description: '搭建 Rust 工具链、Node.js、pnpm 与本地测试验证环境。',
    link: '/zh/development/setup',
  },
  {
    title: '运行时架构',
    description: '服务生命周期、Tokio 任务边界、调度器状态与后端 Rust 接口约定。',
    link: '/zh/development/architecture',
  },
  {
    title: '配置解析',
    description: '四层配置继承在运行时的合并、缓存与校验实现机制。',
    link: '/zh/development/configuration',
  },
  {
    title: '管道执行约定',
    description: '任务队列调度、处理器结果契约、输出暂存提交与重启恢复逻辑。',
    link: '/zh/development/pipeline',
  },
  {
    title: '录制引擎内部实现',
    description: '下载器接口、子进程监管、协作停止与分段上报内部流程。',
    link: '/zh/development/engines',
  },
  {
    title: 'Mesio 引擎',
    description: '深入了解原生 Rust FLV、HLS 与 DASH 下载及实时流修复流水线。',
    link: '/zh/concepts/mesio',
  },
  {
    title: '通知内部实现',
    description: '多渠道并行投递、队列限流、本地化渲染与后端通知接口。',
    link: '/zh/development/notifications',
  },
  {
    title: '持久化约定',
    description: 'SQLite 连接池预算、数据库迁移、事务边界与仓库写入规范。',
    link: '/zh/development/persistence',
  },
  {
    title: '监控内部实现',
    description: '健康探测、文件系统采样、GPU 诊断与日志归档流式下载实现。',
    link: '/zh/development/monitoring',
  },
]
</script>

# 开发

供修改录制器的贡献者使用。先搭建[开发环境](/zh/development/setup)，然后了解运行时架构，再查看正在修改的子系统约定。

<DocCardGrid :items="items" />
