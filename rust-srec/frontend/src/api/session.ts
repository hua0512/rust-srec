import { queryOptions } from '@tanstack/react-query'
import { checkAuthFn } from '@/server/functions'

export const sessionQueryOptions = queryOptions({
  queryKey: ['session'],
  queryFn: async () => {
    try {
      const result = await checkAuthFn()
      return result
    } catch (e) {
      console.error('Session check failed', e)
      return null
    }
  },
})
