<script setup>
const items = [
  {
    title: '设置',
    description: '全局、平台、模板、保留策略、弹幕统计与代理设置的完整字段说明。',
    link: '/zh/reference/settings',
  },
  {
    title: '环境变量',
    description: '通过环境变量配置端口、存储路径、认证、关闭超时、CORS 与日志行为。',
    link: '/zh/reference/environment',
  },
  {
    title: '文件名模板',
    description: '录制输出路径与文件名的占位符、日期格式化及非法字符清理规则。',
    link: '/zh/reference/filenames',
  },
  {
    title: '覆盖配置',
    description: '平台选项、凭据选择与代理路由的 JSON 覆盖格式说明。',
    link: '/zh/reference/configuration-overrides',
  },
  {
    title: '处理器',
    description: '转封装、转码、缩略图、rclone、百度网盘、压缩归档及自定义命令处理器参数。',
    link: '/zh/reference/processors',
  },
  {
    title: '工作流定义',
    description: '自定义 DAG 工作流的步骤依赖、重试策略、超时与执行语义说明。',
    link: '/zh/reference/workflows',
  },
  {
    title: 'REST API',
    description: '接口认证、路由分组、搜索筛选规则、错误格式与 OpenAPI 文档入口。',
    link: '/zh/api/',
  },
  {
    title: 'API 密钥与 MCP',
    description: '创建分级权限 API 密钥，并将 AI 客户端接入内置 MCP 服务。',
    link: '/zh/api/api-keys-mcp',
  },
  {
    title: '平台支持',
    description: '各直播平台的功能矩阵、登录方式、批量检测及提取参数说明。',
    link: '/zh/platforms/',
  },
]
</script>

# 参考

查询配置字段、处理器选项、平台支持明细与 API 行为规范。

<DocCardGrid :items="items" />
