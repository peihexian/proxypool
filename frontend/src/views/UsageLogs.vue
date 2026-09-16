<template>
  <div>
    <div class="page-head">
      <div>
        <div class="page-title">使用日志</div>
        <div class="muted">最近 100 次已完成的代理请求（连接结束后记账，长连接会在结束后才出现）</div>
      </div>
      <div class="page-head-actions">
        <el-button @click="load(true)" :loading="loading">刷新</el-button>
      </div>
    </div>
    <el-table :data="list" border stripe style="width: 100%" empty-text="暂无使用记录">
      <el-table-column label="时间" width="180">
        <template #default="{ row }">{{ formatTime(row.ts) }}</template>
      </el-table-column>
      <el-table-column label="客户端 IP" min-width="140">
        <template #default="{ row }">
          <span class="mono">{{ row.client_ip || '-' }}</span>
        </template>
      </el-table-column>
      <el-table-column label="代理 IP" min-width="150">
        <template #default="{ row }">
          <span class="mono">{{ row.proxy_ip || '-' }}</span>
        </template>
      </el-table-column>
      <el-table-column label="目标" min-width="200" show-overflow-tooltip>
        <template #default="{ row }">
          <span class="mono">{{ row.dest || '-' }}</span>
        </template>
      </el-table-column>
      <el-table-column label="协议" width="100">
        <template #default="{ row }">
          <el-tag v-if="row.protocol" size="small" :type="protoType(row.protocol)">
            {{ row.protocol.toUpperCase() }}
          </el-tag>
          <span v-else>-</span>
        </template>
      </el-table-column>
      <el-table-column label="上行" width="110">
        <template #default="{ row }">{{ formatBytes(row.bytes_up) }}</template>
      </el-table-column>
      <el-table-column label="下行" width="110">
        <template #default="{ row }">{{ formatBytes(row.bytes_down) }}</template>
      </el-table-column>
      <el-table-column prop="service_name" label="服务" min-width="120" show-overflow-tooltip />
    </el-table>
  </div>
</template>

<script setup lang="ts">
import { onMounted, onUnmounted, ref } from 'vue'
import { fetchUsageLogs } from '../api'
import { formatBytes, formatTime } from '../format'

const list = ref<any[]>([])
const loading = ref(false)
let timer: ReturnType<typeof setInterval> | null = null

function protoType(p: string) {
  if (p === 'socks5h') return 'warning'
  if (p === 'socks5') return ''
  return 'success'
}

async function load(showLoading = false) {
  if (showLoading) loading.value = true
  try {
    const data: any = await fetchUsageLogs()
    list.value = data.items || []
  } finally {
    if (showLoading) loading.value = false
  }
}

onMounted(async () => {
  await load(true)
  timer = setInterval(() => load(false), 5000)
})
onUnmounted(() => {
  if (timer) clearInterval(timer)
})
</script>
