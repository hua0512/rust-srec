export const getBaseUrl = () => {
  // Check for server-side environment variables first (for SSR)
  if (typeof process !== 'undefined' && process.env) {
    if (process.env.API_BASE_URL) return process.env.API_BASE_URL;
    if (process.env.BACKEND_URL) return `${process.env.BACKEND_URL}/api`;
  }
  // Fallback to relative path for client-side or if no env var is set
  return import.meta.env.VITE_API_BASE_URL || '/api';
};

export const BASE_URL = getBaseUrl();
