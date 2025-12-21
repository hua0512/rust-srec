// Determine the base URL for the backend API.
// Priority:
// 1. process.env.API_BASE_URL (Runtime env var, ideal for Docker)
// 2. import.meta.env.VITE_API_BASE_URL (Build time env var)
// 3. Fallback to localhost default
export const getBaseUrl = () => {
  if (typeof process !== 'undefined' && process.env.API_BASE_URL) {
    return process.env.API_BASE_URL;
  }
  return import.meta.env.VITE_API_BASE_URL || 'http://127.0.0.1:12555/api';
};

export const BASE_URL = getBaseUrl();
