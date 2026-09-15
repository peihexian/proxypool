import { createRouter, createWebHistory } from 'vue-router'

const router = createRouter({
  history: createWebHistory(),
  routes: [
    { path: '/login', component: () => import('./views/Login.vue') },
    {
      path: '/',
      component: () => import('./views/Layout.vue'),
      children: [
        { path: '', component: () => import('./views/Dashboard.vue') },
        { path: 'pools', component: () => import('./views/Pools.vue') },
        { path: 'pools/:id', component: () => import('./views/PoolDetail.vue') },
        { path: 'services', component: () => import('./views/Services.vue') },
        { path: 'policies', component: () => import('./views/Policies.vue') },
        { path: 'logs', component: () => import('./views/UsageLogs.vue') },
        { path: 'settings', component: () => import('./views/Settings.vue') },
      ],
    },
  ],
})

router.beforeEach((to) => {
  const token = localStorage.getItem('token')
  if (to.path !== '/login' && !token) return '/login'
  if (to.path === '/login' && token) return '/'
})

export default router
