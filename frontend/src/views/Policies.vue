<template>
  <div>
    <div class="mb-4 flex items-center justify-between">
      <div>
        <div class="page-title">检测策略</div>
        <div class="muted">延迟探测、失败退避与隔离策略，可被节点池引用</div>
      </div>
      <el-button type="primary" @click="open()">新增策略</el-button>
    </div>
    <el-table :data="list" border stripe>
      <el-table-column prop="name" label="名称" min-width="140" />
      <el-table-column label="检测间隔" width="110">
        <template #default="{ row }">{{ row.check_interval }}s</template>
      </el-table-column>
      <el-table-column prop="check_url" label="检测地址" min-width="220" show-overflow-tooltip />
      <el-table-column label="超时" width="90">
        <template #default="{ row }">{{ row.timeout_ms }}ms</template>
      </el-table-column>
      <el-table-column label="失败处理" width="140">
        <template #default="{ row }">{{ failLabel(row.on_fail) }}</template>
      </el-table-column>
      <el-table-column prop="max_fails" label="最大失败" width="100" />
      <el-table-column label="操作" width="160" fixed="right">
        <template #default="{ row }">
          <el-button text type="primary" @click="open(row)">编辑</el-button>
          <el-button text type="danger" @click="remove(row)">删除</el-button>
        </template>
      </el-table-column>
    </el-table>

    <el-dialog v-model="visible" :title="form.id ? '编辑策略' : '新增策略'" width="560px">
      <el-form :model="form" label-width="130px">
        <el-form-item label="名称" required>
          <el-input v-model="form.name" />
        </el-form-item>
        <el-form-item label="检测间隔(秒)">
          <el-input-number v-model="form.check_interval" :min="30" :max="86400" />
        </el-form-item>
        <el-form-item label="检测 URL">
          <el-input v-model="form.check_url" placeholder="https://api.ipify.org" />
        </el-form-item>
        <el-form-item label="超时(ms)">
          <el-input-number v-model="form.timeout_ms" :min="1000" :max="60000" :step="500" />
        </el-form-item>
        <el-form-item label="失败处理">
          <el-select v-model="form.on_fail" class="w-full">
            <el-option label="指数退避后重试" value="backoff" />
            <el-option label="永久隔离该账号" value="isolate" />
            <el-option label="临时隔离后再检测" value="temp_isolate" />
          </el-select>
        </el-form-item>
        <el-form-item v-if="form.on_fail === 'temp_isolate'" label="临时隔离(秒)">
          <el-input-number v-model="form.isolate_seconds" :min="30" :max="86400" />
        </el-form-item>
        <el-form-item v-if="form.on_fail === 'backoff'" label="退避基数(秒)">
          <el-input-number v-model="form.backoff_base_seconds" :min="5" :max="3600" />
        </el-form-item>
        <el-form-item label="连续失败阈值">
          <el-input-number v-model="form.max_fails" :min="1" :max="20" />
        </el-form-item>
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
import { createPolicy, deletePolicy, fetchPolicies, updatePolicy } from '../api'

const list = ref<any[]>([])
const visible = ref(false)
const saving = ref(false)
const form = reactive<any>(empty())

function empty() {
  return {
    id: '',
    name: '',
    check_interval: 300,
    check_url: 'https://api.ipify.org',
    timeout_ms: 10000,
    on_fail: 'temp_isolate',
    isolate_seconds: 600,
    backoff_base_seconds: 30,
    max_fails: 3,
  }
}

function failLabel(v: string) {
  return { backoff: '指数退避', isolate: '永久隔离', temp_isolate: '临时隔离' }[v] || v
}

async function load() {
  list.value = (await fetchPolicies()) as any
}

function open(row?: any) {
  Object.assign(form, empty(), row || {})
  visible.value = true
}

async function save() {
  saving.value = true
  try {
    const payload = { ...form }
    if (form.id) await updatePolicy(form.id, payload)
    else await createPolicy(payload)
    ElMessage.success('已保存')
    visible.value = false
    await load()
  } finally {
    saving.value = false
  }
}

async function remove(row: any) {
  await ElMessageBox.confirm(`删除策略「${row.name}」？`, '确认')
  await deletePolicy(row.id)
  ElMessage.success('已删除')
  await load()
}

onMounted(load)
</script>
