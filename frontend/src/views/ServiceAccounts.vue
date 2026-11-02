<template>
  <div>
    <div class="page-head">
      <div>
        <el-button text @click="$router.push('/services')">← 返回服务节点</el-button>
        <div class="page-title">{{ service.name || '批量账号' }}</div>
        <div class="muted">
          每个账号固定绑定一个通过检测的出口 IP；IP 池更新或节点失效后会自动重新分配，IP 不够时才复用并尽量均摊。
        </div>
      </div>
      <div class="page-head-actions">
        <el-button type="primary" @click="openBatch">批量分配</el-button>
        <el-button @click="copyAccounts">复制账号密码</el-button>
        <el-button @click="copyProxies">复制代理链接</el-button>
        <el-button @click="changeAllPasswords">统一改密</el-button>
        <el-button @click="reassign">重新分配</el-button>
        <el-button type="danger" plain @click="clearAll">清空账号</el-button>
      </div>
    </div>

    <div class="grid grid-cols-2 lg:grid-cols-4 gap-3 mb-4">
      <el-card class="stat-card" shadow="never">
        <div class="muted">账号总数</div>
        <div class="text-2xl font-semibold">{{ accounts.length }}</div>
      </el-card>
      <el-card class="stat-card" shadow="never">
        <div class="muted">已绑定 IP</div>
        <div class="text-2xl font-semibold">{{ boundCount }}</div>
      </el-card>
      <el-card class="stat-card" shadow="never">
        <div class="muted">唯一出口 IP</div>
        <div class="text-2xl font-semibold">{{ uniqueIps }}</div>
      </el-card>
      <el-card class="stat-card" shadow="never">
        <div class="muted">可用节点</div>
        <div class="text-2xl font-semibold">{{ availableNodes }}</div>
      </el-card>
    </div>

    <el-alert
      v-if="!availableNodes"
      class="mb-3"
      type="warning"
      :closable="false"
      show-icon
      title="当前没有已通过检测的可用节点，账号会保持“待分配”，等节点恢复后自动绑定。"
    />
    <el-alert
      v-else-if="reusedCount > 0"
      class="mb-3"
      type="info"
      :closable="false"
      show-icon
      :title="`出口 IP 不足：${boundCount} 个已绑定账号分布在 ${uniqueIps} 个 IP 上，有 ${reusedCount} 个账号在复用。`"
    />
    <el-alert
      v-else-if="boundCount > 0 && boundCount === uniqueIps"
      class="mb-3"
      type="success"
      :closable="false"
      show-icon
      :title="`资源充足：${boundCount} 个账号各自独占一个出口 IP。`"
    />

    <div class="filter-bar mb-3">
      <el-input v-model="q" placeholder="搜索账号 / 出口 IP / 节点" clearable class="filter-input" />
      <el-select v-model="statusFilter" class="filter-select">
        <el-option label="全部账号" value="all" />
        <el-option label="已绑定" value="bound" />
        <el-option label="待分配" value="unassigned" />
        <el-option label="已停用" value="disabled" />
      </el-select>
      <el-switch v-model="showPassword" active-text="显示密码" />
      <el-button @click="load()">刷新</el-button>
    </div>

    <el-table
      v-if="!isMobile"
      v-loading="loading"
      :data="filteredAccounts"
      border
      stripe
      style="width: 100%"
    >
      <el-table-column prop="username" label="账号" min-width="130">
        <template #default="{ row }">
          <span class="mono">{{ row.username }}</span>
        </template>
      </el-table-column>
      <el-table-column label="密码" width="160">
        <template #default="{ row }">
          <span class="mono">{{ showPassword ? row.password : '••••••' }}</span>
        </template>
      </el-table-column>
      <el-table-column label="出口 IP" width="190">
        <template #default="{ row }">
          <span class="mono">{{ displayIp(row) }}</span>
          <el-tag v-if="isShared(row)" size="small" type="warning" class="ml-1">共用</el-tag>
        </template>
      </el-table-column>
      <el-table-column label="上游节点" min-width="210">
        <template #default="{ row }">
          <template v-if="row.node_host">
            <el-tag size="small" type="info" class="mr-1">{{ (row.node_protocol || '').toUpperCase() }}</el-tag>
            <span class="mono">{{ row.node_host }}:{{ row.node_port }}</span>
            <span v-if="row.node_country_code" class="muted ml-1">{{ row.node_country_code }}</span>
          </template>
          <span v-else class="muted">-</span>
        </template>
      </el-table-column>
      <el-table-column label="延迟" width="90">
        <template #default="{ row }">
          {{ row.node_latency_ms != null ? row.node_latency_ms + 'ms' : '-' }}
        </template>
      </el-table-column>
      <el-table-column label="状态" width="110">
        <template #default="{ row }">
          <el-tag :type="rowStatus(row).type" size="small">{{ rowStatus(row).text }}</el-tag>
        </template>
      </el-table-column>
      <el-table-column label="启用" width="80">
        <template #default="{ row }">
          <el-switch :model-value="row.enabled === 1" @change="onToggle(row, $event)" />
        </template>
      </el-table-column>
      <el-table-column label="操作" width="90" class-name="ops-col" label-class-name="ops-col">
        <template #default="{ row }">
          <el-button text type="danger" size="small" @click="remove(row)">删除</el-button>
        </template>
      </el-table-column>
    </el-table>

    <div v-if="isMobile" class="mobile-list" v-loading="loading">
      <div v-if="!filteredAccounts.length" class="m-empty">暂无账号</div>
      <div v-for="row in filteredAccounts" :key="row.id" class="m-card">
        <div class="m-card-head">
          <div>
            <div class="m-card-title mono">{{ row.username }}</div>
            <div class="m-card-sub mono">{{ showPassword ? row.password : '••••••' }}</div>
          </div>
          <el-tag :type="rowStatus(row).type" size="small">{{ rowStatus(row).text }}</el-tag>
        </div>
        <div class="m-card-meta">
          <span>出口 IP <b class="mono">{{ displayIp(row) }}</b></span>
          <span>延迟 <b>{{ row.node_latency_ms != null ? row.node_latency_ms + 'ms' : '-' }}</b></span>
        </div>
        <div v-if="row.node_host" class="m-card-sub mono mt-1">
          {{ (row.node_protocol || '').toUpperCase() }} {{ row.node_host }}:{{ row.node_port }}
          <span v-if="row.node_country_code"> · {{ row.node_country_code }}</span>
          <el-tag v-if="isShared(row)" size="small" type="warning" class="ml-1">共用</el-tag>
        </div>
        <div class="m-card-actions">
          <el-switch
            :model-value="row.enabled === 1"
            active-text="启用"
            @change="(v: any) => onToggle(row, v)"
          />
          <el-button text type="danger" size="small" @click="remove(row)">删除</el-button>
        </div>
      </div>
    </div>

    <el-dialog v-model="batchVisible" title="批量分配账号" width="560px" top="8vh">
      <el-form :model="batch" label-width="120px">
        <el-form-item label="用户名前缀" required>
          <el-input v-model="batch.prefix" placeholder="user" />
        </el-form-item>
        <el-form-item label="起始序号" required>
          <el-input-number v-model="batch.start" :min="1" :max="999999" />
        </el-form-item>
        <el-form-item label="账号数量" required>
          <el-input-number v-model="batch.count" :min="1" :max="500" />
        </el-form-item>
        <el-form-item label="统一密码" required>
          <el-input v-model="batch.password" show-password placeholder="所有新账号使用同一个密码" />
        </el-form-item>
        <el-form-item label="生成预览">
          <span class="mono">{{ batchPreview }}</span>
        </el-form-item>
        <el-form-item label="启用">
          <el-switch v-model="batch.enabled" />
        </el-form-item>
      </el-form>
      <div class="muted">
        从该服务引用的节点池中，只挑状态为“可用”的节点，并按出口 IP 去重后分配。账号数超过可用 IP
        数时会自动复用，尽量均摊到每个 IP。
      </div>
      <template #footer>
        <el-button @click="batchVisible = false">取消</el-button>
        <el-button type="primary" :loading="saving" @click="createBatch">开始分配</el-button>
      </template>
    </el-dialog>
  </div>
</template>

<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, reactive, ref } from 'vue'
import { useRoute } from 'vue-router'
import { ElMessage, ElMessageBox } from 'element-plus'
import {
  clearServiceAccounts,
  createServiceAccounts,
  deleteServiceAccount,
  fetchService,
  fetchServiceAccounts,
  fetchSettings,
  reassignServiceAccounts,
  setServiceAccountsPassword,
  updateServiceAccount,
} from '../api'
import { useMobile } from '../useMedia'

const route = useRoute()
const isMobile = useMobile()
const serviceId = computed(() => String(route.params.id))

const service = ref<any>({})
const accounts = ref<any[]>([])
const serverHost = ref('127.0.0.1')
const loading = ref(false)
const saving = ref(false)
const showPassword = ref(false)
const q = ref('')
const statusFilter = ref('all')
const batchVisible = ref(false)
const batch = reactive({ prefix: 'user', start: 1, count: 20, password: '', enabled: true })
let timer: number | undefined

const availableNodes = computed(() => Number(service.value.available_nodes) || 0)
const boundRows = computed(() => accounts.value.filter((a) => a.enabled === 1 && a.node_id))
const boundCount = computed(() => boundRows.value.length)
const uniqueIps = computed(() => {
  const ips = new Set<string>()
  for (const a of boundRows.value) {
    const ip = egressOf(a)
    if (ip) ips.add(ip)
  }
  return ips.size
})
const reusedCount = computed(() => Math.max(0, boundCount.value - uniqueIps.value))
const duplicateIps = computed(() => {
  const counts: Record<string, number> = {}
  for (const a of boundRows.value) {
    const ip = egressOf(a)
    if (ip) counts[ip] = (counts[ip] || 0) + 1
  }
  return counts
})
const filteredAccounts = computed(() => {
  const kw = q.value.trim().toLowerCase()
  return accounts.value.filter((row) => {
    if (statusFilter.value === 'bound' && (!row.node_id || row.enabled !== 1)) return false
    if (statusFilter.value === 'unassigned' && row.node_id) return false
    if (statusFilter.value === 'disabled' && row.enabled === 1) return false
    if (!kw) return true
    return [row.username, row.exit_ip, row.node_host, row.node_protocol]
      .filter(Boolean)
      .some((s: string) => String(s).toLowerCase().includes(kw))
  })
})
const batchPreview = computed(() => {
  const start = Math.max(1, Number(batch.start) || 1)
  const count = Math.max(1, Math.min(500, Number(batch.count) || 0))
  const prefix = batch.prefix || 'user'
  return `${prefix}${start} … ${prefix}${start + count - 1}`
})

function displayIp(row: any) {
  return egressOf(row) || '-'
}

function egressOf(row: any) {
  const ip = (row.exit_ip || '').trim()
  if (ip) return ip
  if (row.node_host) return `${row.node_host}:${row.node_port}`
  return ''
}

function isShared(row: any) {
  const ip = egressOf(row)
  return row.enabled === 1 && !!ip && (duplicateIps.value[ip] || 0) > 1
}

function rowStatus(row: any) {
  if (row.enabled !== 1) return { type: 'info', text: '已停用' }
  if (!row.node_id) return { type: 'info', text: '待分配' }
  if (row.node_status === 'active') return { type: 'success', text: '已绑定' }
  return { type: 'warning', text: '待重分配' }
}

async function load(silent = false) {
  if (!silent) loading.value = true
  try {
    const [svc, list, settings] = (await Promise.all([
      fetchService(serviceId.value),
      fetchServiceAccounts(serviceId.value),
      fetchSettings(),
    ])) as any
    service.value = svc
    accounts.value = list
    serverHost.value = settings?.server_host || '127.0.0.1'
  } finally {
    loading.value = false
  }
}

function openBatch() {
  batch.prefix = 'user'
  batch.start = 1
  batch.count = 20
  batch.password = service.value.password || ''
  batch.enabled = true
  batchVisible.value = true
}

async function createBatch() {
  if (!batch.prefix.trim()) {
    ElMessage.warning('请填写用户名前缀')
    return
  }
  if (!batch.password) {
    ElMessage.warning('请填写统一密码')
    return
  }
  saving.value = true
  try {
    const r: any = await createServiceAccounts(serviceId.value, {
      prefix: batch.prefix.trim(),
      start: batch.start,
      count: batch.count,
      password: batch.password,
      enabled: batch.enabled,
    })
    ElMessage.success(`已分配 ${r.created_count} 个账号`)
    if (r.skipped_count) {
      ElMessage.warning(`跳过 ${r.skipped_count} 个已存在账号：${r.skipped.join(', ')}`)
    }
    batchVisible.value = false
    await load()
  } finally {
    saving.value = false
  }
}

async function onToggle(row: any, value: any) {
  await updateServiceAccount(serviceId.value, row.id, { enabled: !!value })
  await load()
}

async function remove(row: any) {
  await ElMessageBox.confirm(`删除账号「${row.username}」？`, '确认', { type: 'warning' })
  await deleteServiceAccount(serviceId.value, row.id)
  ElMessage.success('已删除')
  await load()
}

async function clearAll() {
  if (!accounts.value.length) {
    ElMessage.info('当前没有账号')
    return
  }
  await ElMessageBox.confirm(`清空「${service.value.name || ''}」下的全部 ${accounts.value.length} 个账号？`, '确认', {
    type: 'warning',
  })
  const r: any = await clearServiceAccounts(serviceId.value)
  ElMessage.success(`已删除 ${r.deleted} 个账号`)
  await load()
}

async function reassign() {
  const r: any = await reassignServiceAccounts(serviceId.value)
  ElMessage.success(r.changed ? `重新分配完成，更新 ${r.changed} 个账号` : '当前分配已是最优，无需调整')
  await load()
}

async function changeAllPasswords() {
  if (!accounts.value.length) {
    ElMessage.info('当前没有账号')
    return
  }
  const res: any = await ElMessageBox.prompt('输入所有账号使用的新密码', '统一改密', {
    inputType: 'password',
    inputValidator: (v: string) => (v && v.length ? true : '密码不能为空'),
    confirmButtonText: '保存',
    cancelButtonText: '取消',
  })
  const r: any = await setServiceAccountsPassword(serviceId.value, res.value)
  ElMessage.success(`已更新 ${r.updated} 个账号的密码`)
  await load()
}

async function copyText(text: string) {
  if (!text) {
    ElMessage.warning('没有可复制的账号')
    return
  }
  await navigator.clipboard.writeText(text)
  ElMessage.success('已复制')
}

function enabledAccounts() {
  return accounts.value.filter((a) => a.enabled === 1)
}

async function copyAccounts() {
  const lines = enabledAccounts().map((a) => `${a.username}:${a.password}`)
  await copyText(lines.join('\n'))
}

async function copyProxies() {
  const scheme = service.value.enable_http ? 'http' : service.value.enable_socks5h ? 'socks5h' : 'socks5'
  const host = `${serverHost.value}:${service.value.listen_port}`
  const lines = enabledAccounts().map(
    (a) => `${scheme}://${encodeURIComponent(a.username)}:${encodeURIComponent(a.password)}@${host}`,
  )
  await copyText(lines.join('\n'))
}

onMounted(async () => {
  await load()
  timer = window.setInterval(() => load(true), 12000)
})
onBeforeUnmount(() => {
  if (timer) window.clearInterval(timer)
})
</script>

<style scoped>
.stat-card :deep(.el-card__body) {
  padding: 14px 18px;
}
</style>
