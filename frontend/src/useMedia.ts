import { onMounted, onUnmounted, ref, type Ref } from 'vue'

const QUERY = '(max-width: 767px)'

function matches() {
  return typeof window !== 'undefined' && window.matchMedia(QUERY).matches
}

export function useMobile(): Ref<boolean> {
  const isMobile = ref(matches())
  let mql: MediaQueryList | null = null

  function onChange(e: MediaQueryListEvent) {
    isMobile.value = e.matches
  }

  onMounted(() => {
    mql = window.matchMedia(QUERY)
    isMobile.value = mql.matches
    mql.addEventListener('change', onChange)
  })
  onUnmounted(() => {
    mql?.removeEventListener('change', onChange)
  })

  return isMobile
}
