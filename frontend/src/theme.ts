import { computed, readonly, ref } from 'vue'

export type Theme = 'light' | 'dark'

const STORAGE_KEY = 'proxypool.theme'
const DARK_QUERY = '(prefers-color-scheme: dark)'
const THEME_COLOR: Record<Theme, string> = {
  light: '#f3f6fb',
  dark: '#0b1220',
}

function systemTheme(): Theme {
  if (typeof window === 'undefined' || !window.matchMedia) return 'light'
  return window.matchMedia(DARK_QUERY).matches ? 'dark' : 'light'
}

function storedTheme(): Theme | null {
  try {
    const saved = localStorage.getItem(STORAGE_KEY)
    return saved === 'light' || saved === 'dark' ? saved : null
  } catch {
    return null
  }
}

const theme = ref<Theme>(storedTheme() ?? systemTheme())
let initialized = false

/** Apply the theme class / metadata to <html>. */
export function applyTheme(value: Theme) {
  if (typeof document === 'undefined') return
  const root = document.documentElement
  root.classList.toggle('dark', value === 'dark')
  root.dataset.theme = value
  root.style.colorScheme = value
  const meta = document.querySelector<HTMLMetaElement>('meta[name="theme-color"]')
  if (meta) meta.content = THEME_COLOR[value]
}

export function setTheme(value: Theme, persist = true) {
  theme.value = value
  applyTheme(value)
  if (persist) {
    try {
      localStorage.setItem(STORAGE_KEY, value)
    } catch {
      /* ignore private-mode storage failures */
    }
  }
}

export function toggleTheme() {
  setTheme(theme.value === 'dark' ? 'light' : 'dark')
}

/**
 * Call once before mounting. Also follows the OS theme while the user has not
 * explicitly picked a theme in this browser.
 */
export function initTheme() {
  applyTheme(theme.value)
  if (initialized || typeof window === 'undefined' || !window.matchMedia) return
  initialized = true
  const mql = window.matchMedia(DARK_QUERY)
  mql.addEventListener('change', (event) => {
    if (storedTheme()) return
    setTheme(event.matches ? 'dark' : 'light', false)
  })
}

export function useTheme() {
  return {
    theme: readonly(theme),
    isDark: computed(() => theme.value === 'dark'),
    setTheme,
    toggleTheme,
  }
}
