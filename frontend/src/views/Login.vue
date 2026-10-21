<template>
  <div class="login-bg flex items-center justify-center p-4">
    <el-button
      class="login-theme"
      text
      circle
      :title="isDark ? '切换到浅色主题' : '切换到深色主题'"
      :aria-label="isDark ? '切换到浅色主题' : '切换到深色主题'"
      :aria-pressed="isDark"
      @click="toggleTheme"
    >
      <el-icon :size="20">
        <Sunny v-if="isDark" />
        <Moon v-else />
      </el-icon>
    </el-button>
    <div class="login-card w-full max-w-[420px] rounded-2xl p-6 sm:p-8">
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
          />
        </el-form-item>
        <el-button
          type="primary"
          size="large"
          class="w-full"
          native-type="submit"
          :loading="loading"
        >
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
import { useTheme } from '../theme'

const password = ref('admin')
const loading = ref(false)
const router = useRouter()
const { isDark, toggleTheme } = useTheme()

async function onLogin() {
  // 回车与按钮点击都走表单 submit 这一条路径；这里再挡一次重复提交
  if (loading.value) return
  loading.value = true
  try {
    const res: any = await login(password.value)
    localStorage.setItem('token', res.token)
    ElMessage.success('登录成功')
    // 必须等导航完成再收起 loading：首次进入要懒加载看板 chunk（约 1MB），
    // 不等的话按钮已经恢复、页面还停在登录页，用户会以为没登上而重复点击
    await router.push('/')
  } catch {
    /* 拦截器已提示 */
  } finally {
    loading.value = false
  }
}
</script>
