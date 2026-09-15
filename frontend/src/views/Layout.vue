<template>
  <el-container class="h-full">
    <el-aside width="232px" class="layout-side h-full" style="background: var(--sidebar)">
      <div class="px-5 py-5 text-white">
        <div class="flex items-center gap-2">
          <div class="flex h-9 w-9 items-center justify-center rounded-lg bg-cyan-400/20 text-cyan-300 font-black">P</div>
          <div>
            <div class="text-base font-bold tracking-wide">MyProxy</div>
            <div class="text-xs text-slate-400">Proxy Pool Manager</div>
          </div>
        </div>
      </div>
      <el-menu
        :default-active="active"
        router
        background-color="#0b1220"
        text-color="#94a3b8"
        active-text-color="#67e8f9"
      >
        <el-menu-item index="/">
          <el-icon><DataAnalysis /></el-icon>
          <span>首页看板</span>
        </el-menu-item>
        <el-menu-item index="/pools">
          <el-icon><Connection /></el-icon>
          <span>节点池管理</span>
        </el-menu-item>
        <el-menu-item index="/services">
          <el-icon><Monitor /></el-icon>
          <span>服务节点</span>
        </el-menu-item>
        <el-menu-item index="/policies">
          <el-icon><Aim /></el-icon>
          <span>检测策略</span>
        </el-menu-item>
        <el-menu-item index="/settings">
          <el-icon><Setting /></el-icon>
          <span>系统设置</span>
        </el-menu-item>
      </el-menu>
    </el-aside>
    <el-container>
      <el-header class="flex items-center justify-between bg-white border-b" style="height: 56px">
        <div class="text-slate-500 text-sm">{{ title }}</div>
        <el-button text type="danger" @click="logout">退出登录</el-button>
      </el-header>
      <el-main class="p-5">
        <router-view />
      </el-main>
    </el-container>
  </el-container>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import { useRoute, useRouter } from 'vue-router'

const route = useRoute()
const router = useRouter()
const active = computed(() => {
  if (route.path.startsWith('/pools')) return '/pools'
  return route.path
})
const title = computed(() => {
  if (route.path === '/') return '总览'
  if (route.path.startsWith('/pools')) return '节点池'
  if (route.path.startsWith('/services')) return '对外代理服务'
  if (route.path.startsWith('/policies')) return 'IP 检测策略'
  if (route.path.startsWith('/settings')) return '系统设置'
  return ''
})

function logout() {
  localStorage.removeItem('token')
  router.push('/login')
}
</script>
