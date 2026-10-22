export function createServerFn(): never {
  throw new Error('TanStack Start is not available in desktop SPA builds');
}

export function getGlobalStartContext(): undefined {
  return undefined;
}

export function getRequest(): never {
  throw new Error(
    'TanStack Start server requests are not available in desktop builds',
  );
}

export function getCookie(): never {
  throw new Error(
    'TanStack Start server cookies are not available in desktop builds',
  );
}

export function setCookie(): never {
  throw new Error(
    'TanStack Start server cookies are not available in desktop builds',
  );
}

export function deleteCookie(): never {
  throw new Error(
    'TanStack Start server cookies are not available in desktop builds',
  );
}

export function setResponseHeader(): never {
  throw new Error(
    'TanStack Start server headers are not available in desktop builds',
  );
}
