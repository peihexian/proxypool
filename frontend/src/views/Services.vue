<template>
  <div>
    <div class="page-head">
      <div>
        <div class="page-title">服务节点</div>
        <div class="muted">对外提供 HTTP / SOCKS5 / SOCKS5H，可按国家、ASN、家庭宽带过滤</div>
      </div>
      <div class="page-head-actions">
        <el-button type="primary" @click="open()">新增服务</el-button>
      </div>
    </div>
    <el-table :data="list" border stripe style="width: 100%">
      <el-table-column prop="name" label="名称" min-width="140" />
      <el-table-column label="监听" width="150">
        <template #default="{ row }">
          <span class="mono">{{ row.listen_host }}:{{ row.listen_port }}</span>
        </template>
      </el-table-column>
      <el-table-column label="协议" width="200">
        <template #default="{ row }">
          <el-tag v-if="row.enable_http" size="small" class="mr-1" type="success">HTTP</el-tag>
          <el-tag v-if="row.enable_socks5" size="small" class="mr-1">SOCKS5</el-tag>
          <el-tag v-if="row.enable_socks5h" size="small" type="warning">SOCKS5H</el-tag>
        </template>
      </el-table-column>
      <el-table-column label="可用 IP" width="90" prop="available_nodes" />
      <el-table-column label="策略" width="120">
        <template #default="{ row }">{{ strategyLabel(row.selection_strategy) }}</template>
      </el-table-column>
      <el-table-column label="健康" width="110">
        <template #default="{ row }">
          <el-tag :type="healthType(row.health)" size="small">{{ healthText(row.health) }}</el-tag>
        </template>
      </el-table-column>
      <el-table-column label="启用" width="80">
        <template #default="{ row }">
          <el-switch :model-value="row.enabled" @change="onToggle(row)" />
        </template>
      </el-table-column>
      <el-table-column label="操作" width="240" min-width="240" class-name="ops-col" label-class-name="ops-col">
        <template #default="{ row }">
          <div class="ops-btns">
            <el-button text type="primary" size="small" @click="open(row)">编辑</el-button>
            <el-button text size="small" @click="showCurl(row)">复制</el-button>
            <el-button text type="success" size="small" @click="onTest(row)">测试</el-button>
            <el-button text type="danger" size="small" @click="remove(row)">删除</el-button>
          </div>
        </template>
      </el-table-column>
    </el-table>

    <el-dialog v-model="visible" :title="form.id ? '编辑服务节点' : '新增服务节点'" width="680px">
      <el-form :model="form" label-width="140px">
        <el-form-item label="名称" required>
          <el-input v-model="form.name" />
        </el-form-item>
        <el-form-item label="监听地址">
          <el-input v-model="form.listen_host" placeholder="0.0.0.0" />
        </el-form-item>
        <el-form-item label="监听端口" required>
          <el-input-number v-model="form.listen_port" :min="1" :max="65535" />
        </el-form-item>
        <el-form-item label="代理协议">
          <el-checkbox v-model="form.enable_http">HTTP</el-checkbox>
          <el-checkbox v-model="form.enable_socks5">SOCKS5</el-checkbox>
          <el-checkbox v-model="form.enable_socks5h">SOCKS5H</el-checkbox>
        </el-form-item>
        <el-form-item label="访问账号" required>
          <el-input v-model="form.username" />
        </el-form-item>
        <el-form-item label="访问密码" required>
          <el-input v-model="form.password" show-password />
        </el-form-item>
        <el-form-item label="引用节点池">
          <el-select v-model="form.pool_ids" multiple clearable class="w-full" placeholder="空则使用全部启用的节点池">
            <el-option v-for="p in pools" :key="p.id" :label="p.name" :value="p.id" />
          </el-select>
        </el-form-item>
        <el-form-item label="国家过滤">
          <el-select v-model="form.filter_countries" multiple filterable allow-create default-first-option class="w-full" placeholder="如 US、JP，空则不限">
            <el-option v-for="c in countries" :key="c" :label="c" :value="c" />
          </el-select>
        </el-form-item>
        <el-form-item label="ASN 过滤">
          <el-input v-model="asnText" placeholder="逗号分隔，如 7922,701" />
        </el-form-item>
        <el-form-item label="仅家庭宽带">
          <el-switch v-model="form.filter_residential" />
          <span class="muted ml-2">按 ASN 组织名识别 ISP / 家庭宽带</span>
        </el-form-item>
        <el-form-item label="选择策略">
          <el-select v-model="form.selection_strategy" class="w-full">
            <el-option label="轮询" value="round_robin" />
            <el-option label="优先低延迟" value="lowest_latency" />
            <el-option label="优先空闲未使用" value="least_used" />
            <el-option label="粘性会话" value="sticky" />
          </el-select>
        </el-form-item>
        <el-form-item v-if="form.selection_strategy === 'sticky'" label="粘性 TTL(秒)">
          <el-input-number v-model="form.sticky_ttl" :min="30" :max="86400" />
        </el-form-item>
        <el-form-item label="启用">
          <el-switch v-model="form.enabled" />
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button @click="visible = false">取消</el-button>
        <el-button type="primary" :loading="saving" @click="save">保存</el-button>
      </template>
    </el-dialog>

    <el-dialog v-model="curlVisible" title="快速复制 / 测试命令" width="720px">
      <div v-if="curl" class="space-y-3">
        <div v-for="item in curlItems" :key="item.key">
          <div class="muted mb-1">{{ item.label }}</div>
          <div class="copy-row">
            <el-input :model-value="item.value" readonly class="mono" />
            <el-button @click="copy(item.value)">复制</el-button>
          </div>
        </div>
      </div>
    </el-dialog>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, reactive, ref } from 'vue'
import { ElMessage, ElMessageBox } from 'element-plus'
import { createService, deleteService, fetchCurl, fetchPools, fetchServices, testService, toggleService, updateService } from '../api'

const countries = ['US', 'JP', 'KR', 'SG', 'HK', 'TW', 'GB', 'DE', 'FR', 'NL', 'CA', 'AU', 'IN', 'BR', 'CN']
const list = ref<any[]>([])
const pools = ref<any[]>([])
const visible = ref(false)
const saving = ref(false)
const curlVisible = ref(false)
const curl = ref<any>(null)
const asnText = ref('')
const form = reactive<any>(empty())

function empty() {
  return {
    id: '',
    name: '',
    listen_host: '0.0.0.0',
    listen_port: 1080,
    enable_http: true,
    enable_socks5: true,
    enable_socks5h: true,
    username: 'user',
    password: '',
    pool_ids: [] as string[],
    filter_countries: [] as string[],
    filter_asns: [] as number[],
    filter_residential: false,
    selection_strategy: 'round_robin',
    sticky_ttl: 300,
    enabled: true,
  }
}

function strategyLabel(s: string) {
  return ({ round_robin: '轮询', lowest_latency: '低延迟', least_used: '空闲优先', idle_first: '空闲优先', sticky: '粘性会话' } as any)[s] || s
}
function healthType(s: string) {
  return ({ healthy: 'success', error: 'danger', disabled: 'info', stopped: 'warning' } as any)[s] || 'info'
}
function healthText(s: string) {
  return ({ healthy: '监听中', error: '启动失败', disabled: '已禁用', stopped: '未监听' } as any)[s] || s
}

const curlItems = computed(() => {
  if (!curl.value) return []
  return [
    { key: 'http', label: 'HTTP 代理', value: curl.value.http_proxy },
    { key: 'socks', label: 'SOCKS5 代理', value: curl.value.socks5_proxy },
    { key: 'curlh', label: 'curl HTTP', value: curl.value.curl_http },
    { key: 'curls', label: 'curl SOCKS5', value: curl.value.curl_socks5 },
    { key: 'curlsh', label: 'curl SOCKS5H', value: curl.value.curl_socks5h },
  ]
})

async function load() {
  list.value = (await fetchServices()) as any
  pools.value = (await fetchPools()) as any
}

function open(row?: any) {
  Object.assign(form, empty(), row || {})
  asnText.value = (form.filter_asns || []).join(',')
  visible.value = true
}

async function save() {
  saving.value = true
  try {
    form.filter_asns = asnText.value
      .split(/[,，\s]+/)
      .map((s: string) => s.trim())
      .filter(Boolean)
      .map((s: string) => Number(s))
      .filter((n: number) => !Number.isNaN(n))
    if (form.id) await updateService(form.id, form)
    else await createService(form)
    ElMessage.success('已保存')
    visible.value = false
    await load()
  } finally {
    saving.value = false
  }
}

async function onToggle(row: any) {
  await toggleService(row.id)
  await load()
}
async function remove(row: any) {
  await ElMessageBox.confirm(`删除服务「${row.name}」？`, '确认', { type: 'warning' })
  await deleteService(row.id)
  await load()
}
async function showCurl(row: any) {
  curl.value = await fetchCurl(row.id)
  curlVisible.value = true
}
async function onTest(row: any) {
  const r: any = await testService(row.id)
  ElMessage.success(`探测成功，出口 ${r.exit_ip}，延迟 ${r.latency_ms}ms`)
}
async function copy(text: string) {
  await navigator.clipboard.writeText(text)
  ElMessage.success('已复制')
}

onMounted(load)
</script>
