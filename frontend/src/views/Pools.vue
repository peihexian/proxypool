<template>
  <div>
    <div class="page-head">
      <div>
        <div class="page-title">节点池管理</div>
        <div class="muted">远程订阅或本地导入，支持多种协议文本/JSON 格式</div>
      </div>
      <div class="page-head-actions">
        <el-button type="primary" @click="open()">新增节点池</el-button>
      </div>
    </div>
    <el-table :data="list" border stripe style="width: 100%">
      <el-table-column prop="name" label="名称" min-width="140">
        <template #default="{ row }">
          <el-button text type="primary" @click="$router.push(`/pools/${row.id}`)">{{ row.name }}</el-button>
        </template>
      </el-table-column>
      <el-table-column label="来源" width="100">
        <template #default="{ row }">
          <el-tag :type="row.source_type === 'remote' ? 'success' : 'info'" size="small">
            {{ row.source_type === 'remote' ? '远程订阅' : '本地数据' }}
          </el-tag>
        </template>
      </el-table-column>
      <el-table-column prop="data_format" label="格式" width="80" />
      <el-table-column label="节点数" width="90" prop="node_count" />
      <el-table-column label="可用 / 隔离" width="120">
        <template #default="{ row }">{{ row.active_count }} / {{ row.isolated_count }}</template>
      </el-table-column>
      <el-table-column label="启用" width="90">
        <template #default="{ row }">
          <el-switch :model-value="row.enabled" @change="onToggle(row)" />
        </template>
      </el-table-column>
      <el-table-column prop="updated_at" label="更新时间" min-width="180">
        <template #default="{ row }">{{ formatTime(row.updated_at) }}</template>
      </el-table-column>
      <el-table-column label="操作" width="280" min-width="280" class-name="ops-col" label-class-name="ops-col">
        <template #default="{ row }">
          <div class="ops-btns">
            <el-button text type="primary" size="small" @click="$router.push(`/pools/${row.id}`)">节点</el-button>
            <el-button text type="primary" size="small" @click="open(row)">编辑</el-button>
            <el-button v-if="row.source_type === 'remote'" text size="small" @click="onSync(row)">同步</el-button>
            <el-button text size="small" @click="onCheck(row)">检测</el-button>
            <el-button text type="danger" size="small" @click="remove(row)">删除</el-button>
          </div>
        </template>
      </el-table-column>
    </el-table>

    <el-dialog v-model="visible" :title="form.id ? '编辑节点池' : '新增节点池'" width="720px" top="6vh">
      <el-form :model="form" label-width="140px">
        <el-form-item label="名称" required>
          <el-input v-model="form.name" />
        </el-form-item>
        <el-form-item label="节点来源">
          <el-radio-group v-model="form.source_type">
            <el-radio value="local">本地数据</el-radio>
            <el-radio value="remote">远程订阅</el-radio>
          </el-radio-group>
        </el-form-item>
        <el-form-item label="数据格式">
          <el-radio-group v-model="form.data_format">
            <el-radio value="txt">TXT</el-radio>
            <el-radio value="json">JSON</el-radio>
          </el-radio-group>
        </el-form-item>
        <el-form-item v-if="form.data_format === 'txt'" label="节点编码格式">
          <el-select v-model="form.encoding_format" class="w-full">
            <el-option v-for="o in txtFormats" :key="o.value" :label="o.label" :value="o.value" />
          </el-select>
        </el-form-item>
        <template v-else>
          <el-form-item label="JSON 预设">
            <el-select v-model="form.format_config.json_preset" class="w-full">
              <el-option v-for="o in jsonPresets" :key="o.value" :label="o.label" :value="o.value" />
            </el-select>
          </el-form-item>
          <el-form-item v-if="form.format_config.json_preset === 'custom'" label="数组路径">
            <el-input v-model="form.format_config.json_path" placeholder="例如 data.list，空则自动" />
          </el-form-item>
          <el-form-item v-if="form.format_config.json_preset === 'custom'" label="字段映射">
            <div class="grid grid-cols-1 sm:grid-cols-2 gap-2 w-full">
              <el-input v-model="form.format_config.host_field" placeholder="host 字段" />
              <el-input v-model="form.format_config.port_field" placeholder="port 字段" />
              <el-input v-model="form.format_config.user_field" placeholder="username 字段" />
              <el-input v-model="form.format_config.pass_field" placeholder="password 字段" />
            </div>
          </el-form-item>
        </template>
        <el-form-item label="默认协议">
          <el-select v-model="form.default_protocol" class="w-full">
            <el-option label="HTTP" value="http" />
            <el-option label="SOCKS5" value="socks5" />
            <el-option label="SOCKS5H" value="socks5h" />
          </el-select>
        </el-form-item>
        <el-form-item label="字符编码">
          <el-select v-model="form.charset" class="w-full">
            <el-option label="UTF-8" value="utf-8" />
            <el-option label="GBK" value="gbk" />
            <el-option label="GB2312" value="gb2312" />
          </el-select>
        </el-form-item>
        <el-form-item label="检测策略">
          <el-select v-model="form.detection_policy_id" clearable class="w-full" placeholder="使用默认策略">
            <el-option v-for="p in policies" :key="p.id" :label="p.name" :value="p.id" />
          </el-select>
        </el-form-item>
        <el-form-item label="启用">
          <el-switch v-model="form.enabled" />
        </el-form-item>

        <template v-if="form.source_type === 'remote'">
          <el-form-item label="订阅地址" required>
            <el-input v-model="form.subscription_url" placeholder="https://..." />
          </el-form-item>
          <el-form-item label="更新间隔(秒)">
            <el-input-number v-model="form.update_interval" :min="60" :max="86400" />
          </el-form-item>
        </template>
        <template v-else>
          <el-form-item label="导入模式">
            <el-radio-group v-model="form.import_mode">
              <el-radio value="replace">覆盖现有节点</el-radio>
              <el-radio value="append">追加</el-radio>
            </el-radio-group>
          </el-form-item>
          <el-form-item label="节点数据">
            <el-input
              v-model="form.local_data"
              type="textarea"
              :rows="8"
              placeholder="粘贴或输入代理节点，每行一条"
            />
            <div class="mt-2">
              <input ref="fileRef" type="file" accept=".txt,.json,.csv,.list" class="hidden" @change="onFile" />
              <el-button @click="(fileRef as HTMLInputElement).click()">选择文件导入</el-button>
            </div>
          </el-form-item>
        </template>
      </el-form>
      <template #footer>
        <el-button @click="visible = false">取消</el-button>
        <el-button type="primary" :loading="saving" @click="save">保存</el-button>
      </template>
    </el-dialog>
  </div>
</template>

<script setup lang="ts">
import { onMounted, reactive, ref } from 'vue'
import { ElMessage, ElMessageBox } from 'element-plus'
import { checkPool, createPool, deletePool, fetchPolicies, fetchPools, syncPool, togglePool, updatePool } from '../api'
import { formatTime } from '../format'

const txtFormats = [
  { value: 'auto', label: '自动识别' },
  { value: 'host:port', label: 'host:port' },
  { value: 'user:pass@host:port', label: 'username:password@host:port' },
  { value: 'host:port:user:pass', label: 'host:port:username:password' },
  { value: 'scheme://user:pass@host:port', label: 'socks5://username:password@host:port' },
  { value: 'host:port@user:pass', label: 'host:port@username:password' },
  { value: 'user:pass:host:port', label: 'username:password:host:port' },
]
const jsonPresets = [
  { value: 'auto', label: '自动识别（数组/Clash/嵌套 data）' },
  { value: 'ip_port', label: '[{ ip, port, user, pass }]' },
  { value: 'clash', label: 'Clash { proxies: [{ server, port, type }] }' },
  { value: 'string_array', label: '字符串数组 ["host:port", ...]' },
  { value: 'custom', label: '自定义字段映射' },
]

const list = ref<any[]>([])
const policies = ref<any[]>([])
const visible = ref(false)
const saving = ref(false)
const fileRef = ref<HTMLInputElement>()
const form = reactive<any>(empty())

function empty() {
  return {
    id: '',
    name: '',
    source_type: 'local',
    subscription_url: '',
    data_format: 'txt',
    encoding_format: 'auto',
    format_config: { json_preset: 'auto', json_path: '', host_field: 'host', port_field: 'port', user_field: 'username', pass_field: 'password' },
    charset: 'utf-8',
    default_protocol: 'http',
    update_interval: 3600,
    detection_policy_id: '',
    enabled: true,
    local_data: '',
    import_mode: 'replace',
  }
}

async function load() {
  list.value = (await fetchPools()) as any
  policies.value = (await fetchPolicies()) as any
}

function open(row?: any) {
  Object.assign(form, empty())
  if (row) {
    Object.assign(form, row, {
      format_config: row.format_config || empty().format_config,
      local_data: '',
    })
  }
  visible.value = true
}

function onFile(e: Event) {
  const f = (e.target as HTMLInputElement).files?.[0]
  if (!f) return
  const reader = new FileReader()
  reader.onload = () => {
    form.local_data = String(reader.result || '')
  }
  reader.readAsText(f)
}

async function save() {
  saving.value = true
  try {
    const payload = { ...form }
    if (form.id) await updatePool(form.id, payload)
    else await createPool(payload)
    ElMessage.success('已保存')
    visible.value = false
    await load()
  } finally {
    saving.value = false
  }
}

async function onToggle(row: any) {
  await togglePool(row.id)
  await load()
}
async function onSync(row: any) {
  const r: any = await syncPool(row.id)
  ElMessage.success(`同步完成，导入 ${r.imported} 条`)
  await load()
}
async function onCheck(row: any) {
  await checkPool(row.id)
  ElMessage.success('已开始后台检测')
}
async function remove(row: any) {
  await ElMessageBox.confirm(`删除节点池「${row.name}」及其全部节点？`, '确认', { type: 'warning' })
  await deletePool(row.id)
  ElMessage.success('已删除')
  await load()
}

onMounted(load)
</script>
