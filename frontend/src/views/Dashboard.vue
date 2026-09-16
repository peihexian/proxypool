<template>
  <div>
    <div class="mb-4">
      <div class="page-title">首页看板</div>
      <div class="muted">节点规模、隔离情况与近 8 小时流量</div>
    </div>

    <el-row :gutter="16" class="mb-4 dash-row">
      <el-col :xs="24" :sm="12" :md="6" v-for="s in stats" :key="s.label">
        <el-card class="stat-card" shadow="hover">
          <div class="muted">{{ s.label }}</div>
          <div class="mt-2 text-2xl font-bold">{{ s.value }}</div>
          <div class="mt-1 text-xs text-slate-400">{{ s.sub }}</div>
        </el-card>
      </el-col>
    </el-row>

    <el-row :gutter="16" class="dash-row">
      <el-col :xs="24" :md="16">
        <el-card shadow="never">
          <template #header>近 8 小时流量消耗</template>
          <div ref="chartRef" class="chart-box"></div>
        </el-card>
      </el-col>
      <el-col :xs="24" :md="8">
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
import { formatBytes } from '../format'

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

function renderChart() {
  if (!chartRef.value) return
  if (!chart) chart = echarts.init(chartRef.value)
  const hours = data.value.hours || []
  const mobile = window.innerWidth < 768
  chart.setOption({
    tooltip: { trigger: 'axis' },
    legend: { data: ['上传', '下载'] },
    grid: { left: mobile ? 8 : 48, right: 12, top: 32, bottom: mobile ? 8 : 32, containLabel: true },
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
