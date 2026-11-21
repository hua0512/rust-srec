<script setup>
const items = [
  {
    title: '生产部署',
    description: '单节点生产参考拓扑、反向代理配置与常驻进程管理实践。',
    link: '/zh/operations/production',
  },
  {
    title: '存储与容量',
    description: '估算录制磁盘占用、规划输出根目录、配置可写探测并预防磁盘写满。',
    link: '/zh/operations/storage',
  },
  {
    title: '备份与恢复',
    description: '结合配置导出与文件系统快照，保护配置、运行历史与媒体产物。',
    link: '/zh/operations/backup-restore',
  },
  {
    title: '监控',
    description: '监控健康检查接口、日志轮转、SQLite 内存预算与调度器重启上限。',
    link: '/zh/operations/monitoring',
  },
  {
    title: '升级与回滚',
    description: '安全升级 Docker、systemd 或二进制部署，并利用升级前快照可靠回滚。',
    link: '/zh/operations/upgrading',
  },
  {
    title: '安全',
    description: '保护登录会话、API 密钥、平台 Cookie、反向代理与主机敏感文件。',
    link: '/zh/operations/security',
  },
  {
    title: '数据治理',
    description: '梳理录制数据清单、保留周期边界与运营方的合规责任。',
    link: '/zh/operations/data-governance',
  },
  {
    title: '支持与版本',
    description: '了解项目能力边界、版本维护策略以及提交有效问题反馈的方式。',
    link: '/zh/operations/support',
  },
]
</script>

# 运维

面向生产环境的部署手册与检查清单，覆盖存储规划、备份恢复、监控、安全与版本升级。

<DocCardGrid :items="items" />
