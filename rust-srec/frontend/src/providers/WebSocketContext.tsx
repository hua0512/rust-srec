import { createContext, useContext } from 'react';

export interface WebSocketContextType {
  isConnected: boolean;
  subscribe: (streamerId: string) => void;
  unsubscribe: (streamerId: string) => void;
}

export const WebSocketContext = createContext<WebSocketContextType | null>(
  null,
);

export function useWebSocket() {
  const context = useContext(WebSocketContext);
  if (!context) {
    throw new Error('useWebSocket must be used within a WebSocketProvider');
  }
  return context;
}
