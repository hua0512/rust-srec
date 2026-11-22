import React from 'react';

import { createFrontendFailure, FatalScreen } from './fatal-screen';

interface DesktopErrorBoundaryProps {
  children: React.ReactNode;
}

interface DesktopErrorBoundaryState {
  error: unknown;
  hasError: boolean;
}

/**
 * Catches React render errors in the desktop webview and presents the recovery
 * screen so the user can reload the interface without terminating the backend.
 */
export class DesktopErrorBoundary extends React.Component<
  DesktopErrorBoundaryProps,
  DesktopErrorBoundaryState
> {
  override state: DesktopErrorBoundaryState = {
    error: null,
    hasError: false,
  };

  static getDerivedStateFromError(error: unknown): DesktopErrorBoundaryState {
    return {
      error,
      hasError: true,
    };
  }

  override componentDidCatch(error: Error, info: React.ErrorInfo): void {
    console.error('[desktop] React render error:', error, info);
  }

  override render(): React.ReactNode {
    if (this.state.hasError) {
      return <FatalScreen failure={createFrontendFailure(this.state.error)} />;
    }

    return this.props.children;
  }
}
