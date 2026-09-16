<template>
  <el-container class="h-full layout-root">
    <el-aside v-if="!isMobile" width="232px" class="layout-side h-full">
      <SideBrand />
      <SideMenu :active="active" />
    </el-aside>
    <el-drawer
      v-model="drawer"
      direction="ltr"
      size="232px"
      :with-header="false"
      append-to-body
      class="layout-drawer"
    >
      <div class="layout-side" style="min-height: 100%; background: var(--sidebar)">
        <SideBrand />
        <SideMenu :active="active" />
      </div>
    </el-drawer>
    <el-container class="layout-body">
      <el-header class="layout-header">
        <div class="layout-header-left">
          <el-button v-if="isMobile" class="menu-btn" text @click="drawer = true">
            <el-icon :size="22"><Menu /></el-icon>
          </el-button>
          <div class="layout-header-title">{{ title }}</div>
        </div>
        <el-button text type="danger" @click="logout">{{ isMobile ? '退出' : '退出登录' }}</el-button>
      </el-header>
      <el-main class="layout-main">
        <router-view />
      </el-main>
    </el-container>
  </el-container>
</template>

<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useMobile } from '../useMedia'
import SideBrand from '../components/SideBrand.vue'
import SideMenu from '../components/SideMenu.vue'

const route = useRoute()
const router = useRouter()
const isMobile = useMobile()
const drawer = ref(false)
const active = computed(() => {
  if (route.path.startsWith('/pools')) return '/pools'
  return route.path
})
const title = computed(() => {
  if (route.path === '/') return '总览'
  if (route.path.startsWith('/pools')) return '节点池'
  if (route.path.startsWith('/services')) return '对外代理服务'
  if (route.path.startsWith('/policies')) return 'IP 检测策略'
  if (route.path.startsWith('/logs')) return '使用日志'
  if (route.path.startsWith('/settings')) return '系统设置'
  return ''
})

watch(
  () => route.path,
  () => {
    drawer.value = false
  },
)
watch(isMobile, (mobile) => {
  if (!mobile) drawer.value = false
})

function logout() {
  localStorage.removeItem('token')
  router.push('/login')
}
</script>
