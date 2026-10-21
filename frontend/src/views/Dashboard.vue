<template>
  <div>
    <div class="mb-4">
      <div class="page-title">首页看板</div>
      <div class="muted">节点规模、隔离情况与近 8 小时 / 近 14 天流量</div>
    </div>

    <el-row :gutter="16" class="mb-4 dash-row">
      <el-col :xs="24" :sm="12" :md="6" v-for="s in stats" :key="s.label">
        <el-card class="stat-card" shadow="hover">
          <div class="muted">{{ s.label }}</div>
          <div class="mt-2 text-2xl font-bold">{{ s.value }}</div>
          <div class="mt-1 text-xs text-slate-400 leading-5">{{ s.sub }}</div>
          <div v-if="s.extra" class="text-xs text-slate-400 leading-5">{{ s.extra }}</div>
        </el-card>
      </el-col>
    </el-row>

    <el-row :gutter="16" class="mb-4 dash-row">
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

    <el-row :gutter="16" class="mb-4 dash-row">
      <el-col :xs="24" :md="16">
        <el-card shadow="never">
          <template #header>
            <div class="card-head">
              <span>近 14 天每日流量</span>
              <span class="muted">柱状为上传 / 下载，折线为调用次数</span>
            </div>
          </template>
          <div v-if="!hasDays" class="muted py-10 text-center">暂无流量数据</div>
          <div v-else ref="daysChartRef" class="chart-box"></div>
        </el-card>
      </el-col>
      <el-col :xs="24" :md="8">
        <el-card shadow="never" class="h-full">
          <template #header>近 14 天概览</template>
          <div class="day-summary">
            <div class="summary-hero">
              <div class="muted">14 天合计（含探测）</div>
              <div class="text-2xl font-bold">{{ formatBytes(summary.total) }}</div>
              <div class="muted mt-1">
                上传 {{ formatBytes(summary.up) }} / 下载 {{ formatBytes(summary.down) }}
              </div>
              <div class="muted mt-1">
                其中探测 {{ formatBytes(summary.probe_total) }} · {{ formatCount(summary.probe_requests) }} 次
              </div>
            </div>
            <div class="summary-grid">
              <div>
                <div class="muted">日均流量</div>
                <div class="summary-value">{{ formatBytes(summary.avg) }}</div>
              </div>
              <div>
                <div class="muted">业务调用次数</div>
                <div class="summary-value">{{ formatCount(summary.requests) }}</div>
              </div>
              <div>
                <div class="muted">峰值日</div>
                <div class="summary-value">
                  {{ summary.peak_total ? `${summary.peak_label} · ${formatBytes(summary.peak_total)}` : '-' }}
                </div>
              </div>
              <div>
                <div class="muted">有流量天数</div>
                <div class="summary-value">{{ summary.active_days ?? 0 }} / 14</div>
              </div>
              <div>
                <div class="muted">今日（{{ summary.today_label || '-' }}）</div>
                <div class="summary-value">{{ formatBytes(summary.today_total) }}</div>
              </div>
              <div>
                <div class="muted">今日调用</div>
                <div class="summary-value">{{ formatCount(summary.today_requests) }} 次</div>
              </div>
            </div>
          </div>
        </el-card>
      </el-col>
    </el-row>

    <el-row :gutter="16" class="dash-row">
      <el-col :span="24">
        <el-card shadow="never">
          <template #header>
            <div class="card-head">
              <span>每日明细</span>
              <span class="muted">保留最近 15 天，最新在前</span>
            </div>
          </template>
          <el-table
            v-if="!isMobile"
            :data="dayRows"
            border
            stripe
            style="width: 100%"
            empty-text="暂无流量数据"
            show-summary
            :summary-method="daySummary"
          >
            <el-table-column prop="date" label="日期" min-width="140">
              <template #default="{ row }">
                <span class="mono">{{ row.date }}</span>
                <span class="muted ml-1">{{ row.weekday }}</span>
              </template>
            </el-table-column>
            <el-table-column prop="up" label="上传" min-width="110" align="right">
              <template #default="{ row }">{{ formatBytes(row.up) }}</template>
            </el-table-column>
            <el-table-column prop="down" label="下载" min-width="110" align="right">
              <template #default="{ row }">{{ formatBytes(row.down) }}</template>
            </el-table-column>
            <el-table-column prop="total" label="合计" min-width="110" align="right">
              <template #default="{ row }">
                <span class="font-bold">{{ formatBytes(row.total) }}</span>
              </template>
            </el-table-column>
            <el-table-column prop="requests" label="调用次数" min-width="110" align="right">
              <template #default="{ row }">{{ formatCount(row.requests) }}</template>
            </el-table-column>
            <el-table-column prop="ratio" label="占比" min-width="150">
              <template #default="{ row }">
                <div class="flex items-center gap-2">
                  <el-progress :percentage="share(row.total)" :show-text="false" color="#38bdf8" />
                  <span class="muted shrink-0">{{ share(row.total) }}%</span>
                </div>
              </template>
            </el-table-column>
          </el-table>
          <div v-if="isMobile && !dayRows.length" class="m-empty">暂无流量数据</div>
          <div v-else-if="isMobile" class="mobile-list">
            <div v-for="row in dayRows" :key="row.date" class="m-card">
              <div class="m-card-head">
                <div>
                  <div class="m-card-title">
                    {{ row.date }} <span class="muted font-normal">{{ row.weekday }}</span>
                  </div>
                  <div class="m-card-sub">合计 {{ formatBytes(row.total) }}</div>
                </div>
                <span class="muted shrink-0">{{ share(row.total) }}%</span>
              </div>
              <div class="m-card-meta">
                <span>上传 <b>{{ formatBytes(row.up) }}</b></span>
                <span>下载 <b>{{ formatBytes(row.down) }}</b></span>
                <span>调用 <b>{{ formatCount(row.requests) }}</b></span>
              </div>
              <el-progress class="mt-2" :percentage="share(row.total)" :show-text="false" color="#38bdf8" />
            </div>
          </div>
        </el-card>
      </el-col>
    </el-row>
  </div>
</template>

<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import * as echarts from 'echarts'
import { fetchDashboard } from '../api'
import { formatBytes, formatCount } from '../format'
import { useMobile } from '../useMedia'
import { useTheme } from '../theme'

const data = ref<any>({})
const chartRef = ref<HTMLDivElement>()
const daysChartRef = ref<HTMLDivElement>()
const isMobile = useMobile()
const { isDark } = useTheme()
let chart: echarts.ECharts | null = null
let daysChart: echarts.ECharts | null = null

const chartPalette = computed(() =>
  isDark.value
    ? {
        text: '#94a3b8',
        axis: '#3b4a5f',
        split: '#263244',
        tooltipBg: '#1a2230',
        tooltipBorder: '#2a3545',
        tooltipText: '#e5eaf3',
      }
    : {
        text: '#64748b',
        axis: '#cbd5e1',
        split: '#eef2f7',
        tooltipBg: '#ffffff',
        tooltipBorder: '#e2e8f0',
        tooltipText: '#0f172a',
      },
)

const summary = computed(() => data.value.days_summary || {})

const stats = computed(() => [
  { label: '号池代理 IP', value: data.value.total_nodes ?? 0, sub: `可用 ${data.value.active_nodes ?? 0}`, extra: '' },
  { label: '被隔离数量', value: data.value.isolated_nodes ?? 0, sub: `待检测 ${data.value.pending_nodes ?? 0}`, extra: '' },
  {
    // 与下方「近 14 天概览」同源，保证两处数字一致；总量含探测流量
    label: '近 14 天流量',
    value: formatBytes(summary.value.total),
    sub: `上传 ${formatBytes(summary.value.up)} / 下载 ${formatBytes(summary.value.down)}`,
    extra: `业务调用 ${formatCount(summary.value.requests)} 次`,
  },
  { label: '节点池 / 服务', value: `${data.value.pools ?? 0} / ${data.value.services ?? 0}`, sub: '已启用对外服务数', extra: '' },
])

const top = computed(() => data.value.top_clients || [])
const maxTop = computed(() => Math.max(1, ...top.value.map((x: any) => x.total || 0)))
function pct(n: number) {
  return Math.round((n / maxTop.value) * 100)
}

const days = computed<any[]>(() => data.value.days || [])
const hasDays = computed(() => days.value.some((d) => (d.total || 0) > 0 || (d.requests || 0) > 0))
/** 明细表按日期倒序，最新的一天在最上面。 */
const dayRows = computed(() => [...days.value].reverse())
const daysTotal = computed(() => days.value.reduce((sum, d) => sum + (d.total || 0), 0))
function share(n: number) {
  if (!daysTotal.value) return 0
  return Math.round(((n || 0) / daysTotal.value) * 1000) / 10
}
function daySummary({ columns }: any) {
  const out: string[] = []
  columns.forEach((col: any, i: number) => {
    switch (col.property) {
      case 'date':
        out[i] = '合计'
        break
      case 'up':
        out[i] = formatBytes(summary.value.up)
        break
      case 'down':
        out[i] = formatBytes(summary.value.down)
        break
      case 'total':
        out[i] = formatBytes(summary.value.total)
        break
      case 'requests':
        out[i] = formatCount(summary.value.requests)
        break
      default:
        out[i] = ''
    }
  })
  return out
}

/**
 * 图表 tooltip 专用：统一换算成 MB，避免出现 4194304 这种看不清单位的裸字节。
 * 不足 1 MB 时退到 KB / B，保证小流量也读得出来。
 */
function formatChartBytes(n?: number | null) {
  const v = Number(n) || 0
  if (v >= 1048576) {
    const mb = v / 1048576
    return `${mb.toFixed(mb >= 100 ? 0 : mb >= 10 ? 1 : 2)} MB`
  }
  if (v >= 1024) return `${(v / 1024).toFixed(1)} KB`
  return `${v} B`
}

function renderChart() {
  if (!chartRef.value) return
  if (!chart) chart = echarts.init(chartRef.value)
  const hours = data.value.hours || []
  const mobile = window.innerWidth < 768
  const p = chartPalette.value
  chart.setOption({
    textStyle: { color: p.text },
    tooltip: {
      trigger: 'axis',
      axisPointer: { type: 'shadow' },
      backgroundColor: p.tooltipBg,
      borderColor: p.tooltipBorder,
      textStyle: { color: p.tooltipText },
      formatter: (ps: any) => {
        const h = hours[ps?.[0]?.dataIndex ?? 0]
        if (!h) return ''
        const dot = (i: number) => ps?.[i]?.marker ?? ''
        return [
          h.label,
          `${dot(0)}上传 ${formatChartBytes(h.up)}`,
          `${dot(1)}下载 ${formatChartBytes(h.down)}`,
          `合计 ${formatChartBytes(h.total)}`,
        ].join('<br/>')
      },
    },
    legend: { data: ['上传', '下载'], textStyle: { color: p.text } },
    grid: { left: mobile ? 8 : 48, right: 12, top: 32, bottom: mobile ? 8 : 32, containLabel: true },
    xAxis: {
      type: 'category',
      data: hours.map((h: any) => h.label),
      axisLabel: { color: p.text },
      axisLine: { lineStyle: { color: p.axis } },
      axisTick: { lineStyle: { color: p.axis } },
    },
    yAxis: {
      type: 'value',
      axisLabel: { color: p.text, formatter: (v: number) => formatBytes(v) },
      axisLine: { lineStyle: { color: p.axis } },
      axisTick: { lineStyle: { color: p.axis } },
      splitLine: { lineStyle: { color: p.split } },
    },
    series: [
      { name: '上传', type: 'bar', stack: 't', data: hours.map((h: any) => h.up), itemStyle: { color: '#38bdf8' } },
      { name: '下载', type: 'bar', stack: 't', data: hours.map((h: any) => h.down), itemStyle: { color: '#22d3ee' } },
    ],
  })
}

function renderDaysChart() {
  if (!daysChartRef.value) return
  if (!daysChart) daysChart = echarts.init(daysChartRef.value)
  const rows = days.value
  const mobile = window.innerWidth < 768
  const p = chartPalette.value
  daysChart.setOption({
    textStyle: { color: p.text },
    tooltip: {
      trigger: 'axis',
      backgroundColor: p.tooltipBg,
      borderColor: p.tooltipBorder,
      textStyle: { color: p.tooltipText },
      formatter: (ps: any) => {
        const d = rows[ps?.[0]?.dataIndex ?? 0]
        if (!d) return ''
        return [
          `${d.date} ${d.weekday}`,
          `上传 ${formatChartBytes(d.up)}`,
          `下载 ${formatChartBytes(d.down)}`,
          `合计 ${formatChartBytes(d.total)}`,
          `调用 ${formatCount(d.requests)} 次`,
        ].join('<br/>')
      },
    },
    legend: { data: ['上传', '下载', '调用次数'], textStyle: { color: p.text } },
    grid: { left: mobile ? 8 : 48, right: mobile ? 8 : 52, top: 32, bottom: mobile ? 8 : 32, containLabel: true },
    xAxis: {
      type: 'category',
      data: rows.map((d: any) => d.label),
      axisLabel: { color: p.text, interval: mobile ? 2 : 0 },
      axisLine: { lineStyle: { color: p.axis } },
      axisTick: { lineStyle: { color: p.axis } },
    },
    yAxis: [
      {
        type: 'value',
        axisLabel: { color: p.text, formatter: (v: number) => formatBytes(v) },
        axisLine: { lineStyle: { color: p.axis } },
        axisTick: { lineStyle: { color: p.axis } },
        splitLine: { lineStyle: { color: p.split } },
      },
      {
        type: 'value',
        name: '次',
        nameTextStyle: { color: p.text },
        splitLine: { show: false },
        axisLabel: { color: p.text, formatter: (v: number) => formatCount(v) },
        axisLine: { lineStyle: { color: p.axis } },
        axisTick: { lineStyle: { color: p.axis } },
      },
    ],
    series: [
      { name: '上传', type: 'bar', stack: 'day', data: rows.map((d: any) => d.up), itemStyle: { color: '#38bdf8' } },
      { name: '下载', type: 'bar', stack: 'day', data: rows.map((d: any) => d.down), itemStyle: { color: '#22d3ee' } },
      {
        name: '调用次数',
        type: 'line',
        yAxisIndex: 1,
        smooth: true,
        symbolSize: 6,
        data: rows.map((d: any) => d.requests),
        itemStyle: { color: '#f59e0b' },
        lineStyle: { width: 2, color: '#f59e0b' },
      },
    ],
  })
}

async function load() {
  data.value = await fetchDashboard()
  await nextTick()
  renderChart()
  renderDaysChart()
}

const onResize = () => {
  chart?.resize()
  daysChart?.resize()
}
onMounted(async () => {
  await load()
  window.addEventListener('resize', onResize)
})
watch([isDark, isMobile], async () => {
  await nextTick()
  renderChart()
  renderDaysChart()
})
onUnmounted(() => {
  window.removeEventListener('resize', onResize)
  chart?.dispose()
  daysChart?.dispose()
})
</script>

<style scoped>
.card-head {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 12px;
  flex-wrap: wrap;
}

.day-summary .summary-hero {
  padding-bottom: 12px;
  border-bottom: 1px solid var(--border);
}

.summary-grid {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 12px 16px;
  padding-top: 12px;
}

.summary-value {
  font-size: 15px;
  font-weight: 600;
  line-height: 22px;
}
</style>
