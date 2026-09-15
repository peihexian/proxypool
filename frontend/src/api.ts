import axios from 'axios'
import { ElMessage } from 'element-plus'
import router from './router'

const api = axios.create({
  baseURL: '/api',
  timeout: 60000,
})

api.interceptors.request.use((config) => {
  const token = localStorage.getItem('token')
  if (token) {
    config.headers.Authorization = `Bearer ${token}`
  }
  return config
})

api.interceptors.response.use(
  (res) => res.data,
  (err) => {
    const status = err.response?.status
    const msg = err.response?.data?.error || err.message || '请求失败'
    if (status === 401) {
      localStorage.removeItem('token')
      if (router.currentRoute.value.path !== '/login') {
        router.push('/login')
      }
    } else {
      ElMessage.error(msg)
    }
    return Promise.reject(err)
  },
)

export default api

export const login = (password: string) => api.post('/auth/login', { password })
export const fetchMe = () => api.get('/auth/me')
export const fetchSettings = () => api.get('/settings')
export const saveSettings = (data: any) => api.put('/settings', data)
export const updateGeoip = () => api.post('/settings/geoip/update')

export const fetchPolicies = () => api.get('/policies')
export const createPolicy = (data: any) => api.post('/policies', data)
export const updatePolicy = (id: string, data: any) => api.put(`/policies/${id}`, data)
export const deletePolicy = (id: string) => api.delete(`/policies/${id}`)

export const fetchPools = () => api.get('/pools')
export const fetchPool = (id: string) => api.get(`/pools/${id}`)
export const createPool = (data: any) => api.post('/pools', data)
export const updatePool = (id: string, data: any) => api.put(`/pools/${id}`, data)
export const deletePool = (id: string) => api.delete(`/pools/${id}`)
export const togglePool = (id: string) => api.post(`/pools/${id}/toggle`)
export const syncPool = (id: string) => api.post(`/pools/${id}/sync`)
export const importPool = (id: string, data: any) => api.post(`/pools/${id}/import`, data)
export const checkPool = (id: string) => api.post(`/pools/${id}/check`)
export const fetchNodes = (id: string, params: any) => api.get(`/pools/${id}/nodes`, { params })
export const deleteNode = (id: string) => api.delete(`/nodes/${id}`)
export const setNodeStatus = (id: string, status: string) => api.post(`/nodes/${id}/status`, { status })
export const checkNode = (id: string) => api.post(`/nodes/${id}/check`)

export const fetchServices = () => api.get('/services')
export const createService = (data: any) => api.post('/services', data)
export const updateService = (id: string, data: any) => api.put(`/services/${id}`, data)
export const deleteService = (id: string) => api.delete(`/services/${id}`)
export const toggleService = (id: string) => api.post(`/services/${id}/toggle`)
export const testService = (id: string) => api.post(`/services/${id}/test`)
export const fetchCurl = (id: string) => api.get(`/services/${id}/curl`)
export const fetchDashboard = () => api.get('/dashboard')
