<template>
  <div>
    <div class="mb-4">
      <div class="page-title">首页看板</div>
      <div class="muted">节点规模、隔离情况与近 8 小时流量</div>
    </div>

    <el-row :gutter="16" class="mb-4">
      <el-col :span="6" v-for="s in stats" :key="s.label">
        <el-card class="stat-card" shadow="hover">
          <div class="muted">{{ s.label }}</div>
          <div class="mt-2 text-2xl font-bold">{{ s.value }}</div>
          <div class="mt-1 text-xs text-slate-400">{{ s.sub }}</div>
        </el-card>
      </el-col>
    </el-row>

    <el-row :gutter="16">
      <el-col :span="16">
        <el-card shadow="never">
          <template #header>近 8 小时流量消耗</template>
          <div ref="chartRef" style="height: 320px"></div>
        </el-card>
      </el-col>
      <el-col :span="8">
        <el-card shadow="never">
          <template #header>客户端流量 Top 5</template>
          <div v-if="!top.length" class="muted py-10 text-center">暂无流量数据</div>
          <div v-for="(c, i) in top" :key="c.client_ip" class="mb-4">
            <div class="flex justify-between text-sm mb-1">
              <span class="mono">{{ i + 1 }}. {{ c.client_ip }}</span>
              <span>{{ formatBytes(c.total) }}</span>
            </div>
            <el-progress :percentage="pct(c.total)" :show-text="false" color="#22d3ee" />
          </div>
        </el-card>
      </el-col>
    </el-row>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import * as echarts from 'echarts'
import { fetchDashboard } from '../api'

const data = ref<any>({})
const chartRef = ref<HTMLDivElement>()
let chart: echarts.ECharts | null = null

const stats = computed(() => [
  { label: '号池代理 IP', value: data.value.total_nodes ?? 0, sub: `可用 ${data.value.active_nodes ?? 0}` },
  { label: '被隔离数量', value: data.value.isolated_nodes ?? 0, sub: `待检测 ${data.value.pending_nodes ?? 0}` },
  { label: '总流量消耗', value: formatBytes(data.value.total_bytes ?? 0), sub: `上传 ${formatBytes(data.value.total_up ?? 0)}` },
  { label: '节点池 / 服务', value: `${data.value.pools ?? 0} / ${data.value.services ?? 0}`, sub: '已启用对外服务数' },
])

const top = computed(() => data.value.top_clients || [])
const maxTop = computed(() => Math.max(1, ...top.value.map((x: any) => x.total || 0)))
function pct(n: number) {
  return Math.round((n / maxTop.value) * 100)
}

function formatBytes(n: number) {
  if (!n) return '0 B'
  const u = ['B', 'KB', 'MB', 'GB', 'TB']
  let i = 0
  let v = n
  while (v >= 1024 && i < u.length - 1) {
    v /= 1024
    i++
  }
  return `${v.toFixed(v >= 10 || i === 0 ? 0 : 1)} ${u[i]}`
}

function renderChart() {
  if (!chartRef.value) return
  if (!chart) chart = echarts.init(chartRef.value)
  const hours = data.value.hours || []
  chart.setOption({
    tooltip: { trigger: 'axis' },
    legend: { data: ['上传', '下载'] },
    grid: { left: 48, right: 16, top: 32, bottom: 32 },
    xAxis: { type: 'category', data: hours.map((h: any) => h.label) },
    yAxis: { type: 'value', axisLabel: { formatter: (v: number) => formatBytes(v) } },
    series: [
      { name: '上传', type: 'bar', stack: 't', data: hours.map((h: any) => h.up), itemStyle: { color: '#38bdf8' } },
      { name: '下载', type: 'bar', stack: 't', data: hours.map((h: any) => h.down), itemStyle: { color: '#22d3ee' } },
    ],
  })
}

async function load() {
  data.value = await fetchDashboard()
  renderChart()
}

const onResize = () => chart?.resize()
onMounted(async () => {
  await load()
  window.addEventListener('resize', onResize)
})
onUnmounted(() => {
  window.removeEventListener('resize', onResize)
  chart?.dispose()
})
</script>
