import { createRouter } from '@tanstack/react-router'

import { routeTree } from './routeTree.gen'
import { useAuthStore } from './store/auth'
import { AuthState } from './auth'

// Create a new router instance
export const router = createRouter({
  routeTree,
  context: {
    auth: undefined! as AuthState,
  },
  scrollRestoration: true,
  defaultPreloadStaleTime: 0,
})

// Subscribe to auth changes to invalidate the router
// This allows the redirect logic in _auth.tsx beforeLoad to trigger
useAuthStore.subscribe((state, prevState) => {
  console.log('Router: Auth state update', {
    prev: prevState.isAuthenticated,
    next: state.isAuthenticated,
    isProtected: router.state.matches.some(m => m.routeId === '/_auth')
  });

  if (!state.isAuthenticated) {
    // Check if we're on a protected route (child of /_auth)
    // We check via matches to be robust against URL changes
    const isProtectedRoute = router.state.matches.some(m => m.routeId === '/_auth');

    if (isProtectedRoute) {
      console.log('User unauthenticated on protected route. Forcing navigation to login...');
      // Force navigation directly to avoid invalidation loops
      router.navigate({ to: '/login', search: { redirect: window.location.href } });
    }
  }
})

export const getRouter = () => {
  return router
}

