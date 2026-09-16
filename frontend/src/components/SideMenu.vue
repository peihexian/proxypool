<template>
  <el-menu
    :default-active="active"
    router
    background-color="#0b1220"
    text-color="#94a3b8"
    active-text-color="#67e8f9"
  >
    <el-menu-item v-for="item in items" :key="item.index" :index="item.index">
      <el-icon><component :is="item.icon" /></el-icon>
      <span>{{ item.label }}</span>
    </el-menu-item>
  </el-menu>
</template>

<script setup lang="ts">
import { Aim, Connection, DataAnalysis, Document, Monitor, Setting } from '@element-plus/icons-vue'

defineProps<{ active: string }>()

const items = [
  { index: '/', icon: DataAnalysis, label: '首页看板' },
  { index: '/pools', icon: Connection, label: '节点池管理' },
  { index: '/services', icon: Monitor, label: '服务节点' },
  { index: '/policies', icon: Aim, label: '检测策略' },
  { index: '/logs', icon: Document, label: '使用日志' },
  { index: '/settings', icon: Setting, label: '系统设置' },
]
</script>
