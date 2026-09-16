<template>
  <div class="login-bg flex items-center justify-center p-4">
    <div class="w-full max-w-[420px] rounded-2xl bg-white/95 p-6 sm:p-8 shadow-2xl">
      <div class="mb-6 text-center">
        <div class="mx-auto mb-3 flex h-12 w-12 items-center justify-center rounded-xl bg-slate-900 text-cyan-300 text-xl font-black">
          P
        </div>
        <h1 class="text-2xl font-bold">ProxyPool</h1>
        <p class="muted mt-1">代理池管理系统</p>
      </div>
      <el-form @submit.prevent="onLogin">
        <el-form-item>
          <el-input
            v-model="password"
            type="password"
            size="large"
            show-password
            placeholder="管理员密码（默认 admin）"
            @keyup.enter="onLogin"
          />
        </el-form-item>
        <el-button type="primary" size="large" class="w-full" :loading="loading" @click="onLogin">
          登录
        </el-button>
      </el-form>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref } from 'vue'
import { useRouter } from 'vue-router'
import { ElMessage } from 'element-plus'
import { login } from '../api'

const password = ref('admin')
const loading = ref(false)
const router = useRouter()

async function onLogin() {
  loading.value = true
  try {
    const res: any = await login(password.value)
    localStorage.setItem('token', res.token)
    ElMessage.success('登录成功')
    router.push('/')
  } catch {
    /* interceptor */
  } finally {
    loading.value = false
  }
}
</script>
