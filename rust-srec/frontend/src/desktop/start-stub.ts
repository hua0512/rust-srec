export function createServerFn(): never {
  throw new Error('TanStack Start is not available in desktop SPA builds');
}

export function getGlobalStartContext(): undefined {
  return undefined;
}

export function useSession(): never {
  throw new Error('TanStack Start server session is not available in desktop builds');
}

export function getRequestHeader(): never {
  throw new Error('TanStack Start server headers are not available in desktop builds');
}

export function setResponseHeader(): never {
  throw new Error('TanStack Start server headers are not available in desktop builds');
}
