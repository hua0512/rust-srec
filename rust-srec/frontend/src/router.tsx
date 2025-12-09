import { createRouter } from '@tanstack/react-router'

import { routeTree } from './routeTree.gen'
import { useAuthStore } from './store/auth'

// Create a new router instance
export const router = createRouter({
  routeTree,
  scrollRestoration: true,
  defaultPreloadStaleTime: 0,
})

// Subscribe to auth changes to invalidate the router
// This allows the redirect logic in _auth.tsx beforeLoad to trigger
useAuthStore.subscribe((state, prevState) => {
  if (!state.isAuthenticated && prevState.isAuthenticated) {
    console.log('Navigating to login due to auth change')
    router.navigate({ to: '/login', search: { redirect: window.location.href } })
  }
})

export const getRouter = () => {
  return router
}

