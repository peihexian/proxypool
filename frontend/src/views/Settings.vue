<template>
  <div class="max-w-3xl">
    <div class="mb-4">
      <div class="page-title">系统设置</div>
      <div class="muted">管理员密码、对外展示地址与 GeoIP 数据库</div>
    </div>
    <el-card shadow="never" class="mb-4">
      <template #header>访问与地址</template>
      <el-form label-width="140px">
        <el-form-item label="本机 IP / 域名">
          <el-input v-model="serverHost" placeholder="用于生成 curl 和代理链接" />
        </el-form-item>
        <el-form-item label="原密码">
          <el-input v-model="oldPassword" type="password" show-password />
        </el-form-item>
        <el-form-item label="新密码">
          <el-input v-model="newPassword" type="password" show-password placeholder="不修改请留空" />
        </el-form-item>
        <el-form-item>
          <el-button type="primary" :loading="saving" @click="save">保存设置</el-button>
        </el-form-item>
      </el-form>
    </el-card>
    <el-card shadow="never">
      <template #header>GeoIP 数据库</template>
      <el-descriptions :column="1" border>
        <el-descriptions-item label="Country 库更新时间">
          {{ settings.geoip_country_updated_at || '尚未下载' }}
        </el-descriptions-item>
        <el-descriptions-item label="ASN 库更新时间">
          {{ settings.geoip_asn_updated_at || '尚未下载' }}
        </el-descriptions-item>
      </el-descriptions>
      <div class="mt-4">
        <el-button type="primary" :loading="updating" @click="doUpdate">立即更新 GeoIP</el-button>
        <span class="muted ml-2">将下载 GeoLite2 Country / ASN，用于出口国家与家庭宽带识别</span>
      </div>
    </el-card>
  </div>
</template>

<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { ElMessage } from 'element-plus'
import { fetchSettings, saveSettings, updateGeoip } from '../api'

const settings = ref<any>({})
const serverHost = ref('127.0.0.1')
const oldPassword = ref('')
const newPassword = ref('')
const saving = ref(false)
const updating = ref(false)

async function load() {
  settings.value = await fetchSettings()
  serverHost.value = settings.value.server_host || '127.0.0.1'
}

async function save() {
  saving.value = true
  try {
    const payload: any = { server_host: serverHost.value }
    if (newPassword.value) {
      payload.old_password = oldPassword.value
      payload.new_password = newPassword.value
    }
    await saveSettings(payload)
    ElMessage.success('已保存')
    oldPassword.value = ''
    newPassword.value = ''
    await load()
  } finally {
    saving.value = false
  }
}

async function doUpdate() {
  updating.value = true
  try {
    const r: any = await updateGeoip()
    ElMessage.success('GeoIP 已更新')
    settings.value = { ...settings.value, ...r }
  } finally {
    updating.value = false
  }
}

onMounted(load)
</script>
