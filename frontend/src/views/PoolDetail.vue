<template>
  <div>
    <div class="page-head">
      <div>
        <el-button text @click="$router.push('/pools')">← 返回</el-button>
        <div class="page-title">{{ pool.name || '节点列表' }}</div>
        <div class="muted">出口 IP / 国家 / ASN / 延迟与隔离状态</div>
      </div>
      <div class="page-head-actions">
        <el-button @click="onCheckAll">全部检测</el-button>
        <el-button v-if="pool.source_type === 'remote'" type="primary" @click="onSync">同步订阅</el-button>
      </div>
    </div>
    <div class="filter-bar mb-3">
      <el-input v-model="q" placeholder="搜索 host / 出口 IP / ASN" clearable class="filter-input" @keyup.enter="load" />
      <el-select v-model="status" placeholder="状态" clearable class="filter-select" @change="load">
        <el-option label="全部" value="" />
        <el-option label="可用" value="active" />
        <el-option label="待检测" value="pending" />
        <el-option label="已隔离" value="isolated" />
        <el-option label="已禁用" value="disabled" />
      </el-select>
      <el-button @click="load">查询</el-button>
    </div>
    <el-table :data="items" border stripe v-loading="loading" style="width: 100%">
      <el-table-column prop="protocol" label="协议" width="90" />
      <el-table-column label="节点" min-width="180">
        <template #default="{ row }">
          <span class="mono">{{ row.host }}:{{ row.port }}</span>
        </template>
      </el-table-column>
      <el-table-column prop="username" label="账号" width="120" show-overflow-tooltip />
      <el-table-column prop="exit_ip" label="出口 IP" width="130" />
      <el-table-column label="国家" width="90">
        <template #default="{ row }">{{ row.country_code || '-' }}</template>
      </el-table-column>
      <el-table-column label="ASN" min-width="160">
        <template #default="{ row }">
          <div v-if="row.asn">AS{{ row.asn }}</div>
          <div class="muted">{{ row.asn_org || '-' }}</div>
        </template>
      </el-table-column>
      <el-table-column label="宽带" width="80">
        <template #default="{ row }">
          <el-tag v-if="row.is_residential" type="success" size="small">家庭</el-tag>
          <el-tag v-else type="info" size="small">其他</el-tag>
        </template>
      </el-table-column>
      <el-table-column prop="latency_ms" label="延迟" width="80">
        <template #default="{ row }">{{ row.latency_ms != null ? row.latency_ms + 'ms' : '-' }}</template>
      </el-table-column>
      <el-table-column label="状态" width="100">
        <template #default="{ row }">
          <el-tag :type="statusType(row.status)" size="small">{{ statusText(row.status) }}</el-tag>
        </template>
      </el-table-column>
      <el-table-column label="操作" width="200" min-width="200" class-name="ops-col" label-class-name="ops-col">
        <template #default="{ row }">
          <div class="ops-btns">
            <el-button text type="primary" size="small" @click="onCheck(row)">检测</el-button>
            <el-button v-if="row.status !== 'isolated'" text size="small" @click="setStatus(row, 'isolated')">隔离</el-button>
            <el-button v-else text size="small" @click="setStatus(row, 'pending')">恢复</el-button>
            <el-button text type="danger" size="small" @click="remove(row)">删除</el-button>
          </div>
        </template>
      </el-table-column>
    </el-table>
    <div class="mt-3 flex justify-end pagination-wrap">
      <el-pagination
        background
        :small="isMobile"
        :layout="isMobile ? 'total, prev, next' : 'total, prev, pager, next'"
        :total="total"
        :page-size="pageSize"
        v-model:current-page="page"
        @current-change="load"
      />
    </div>
  </div>
</template>

<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { useRoute } from 'vue-router'
import { ElMessage, ElMessageBox } from 'element-plus'
import { checkNode, checkPool, deleteNode, fetchNodes, fetchPool, setNodeStatus, syncPool } from '../api'
import { useMobile } from '../useMedia'

const route = useRoute()
const isMobile = useMobile()
const pool = ref<any>({})
const items = ref<any[]>([])
const total = ref(0)
const page = ref(1)
const pageSize = 50
const q = ref('')
const status = ref('')
const loading = ref(false)
const id = () => String(route.params.id)

function statusType(s: string) {
  return ({ active: 'success', pending: 'warning', isolated: 'danger', disabled: 'info' } as any)[s] || 'info'
}
function statusText(s: string) {
  return ({ active: '可用', pending: '待检测', isolated: '隔离', disabled: '禁用' } as any)[s] || s
}

async function load() {
  loading.value = true
  try {
    pool.value = await fetchPool(id())
    const res: any = await fetchNodes(id(), { page: page.value, page_size: pageSize, q: q.value, status: status.value })
    items.value = res.items
    total.value = res.total
  } finally {
    loading.value = false
  }
}

async function onSync() {
  const r: any = await syncPool(id())
  ElMessage.success(`同步完成，导入 ${r.imported} 条`)
  await load()
}
async function onCheckAll() {
  await checkPool(id())
  ElMessage.success('已开始后台检测')
}
async function onCheck(row: any) {
  await checkNode(row.id)
  ElMessage.success('检测完成')
  await load()
}
async function setStatus(row: any, s: string) {
  await setNodeStatus(row.id, s)
  await load()
}
async function remove(row: any) {
  await ElMessageBox.confirm('删除该节点？', '确认')
  await deleteNode(row.id)
  await load()
}

onMounted(load)
</script>
