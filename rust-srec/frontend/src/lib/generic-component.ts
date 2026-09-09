import { lazy, memo } from 'react';
import type { ComponentType, ReactNode } from 'react';

/** A component whose props carry type parameters of their own. */
type GenericComponent = (props: never) => ReactNode;

/**
 * Memoizes a component that has generic props.
 *
 * `React.memo` fixes a component's type parameters at the point it wraps the
 * component, which would force every caller onto the constraint instead of its
 * own concrete type. The memoized component behaves identically at runtime, so
 * the original signature is restored on the way out.
 */
export function genericMemo<TComponent extends GenericComponent>(
  component: TComponent,
  displayName?: string,
): TComponent {
  const memoized = memo(component);
  if (displayName) {
    memoized.displayName = displayName;
  }
  return memoized as unknown as TComponent;
}

/**
 * Lazily loads a component that has generic props.
 *
 * `React.lazy` fixes the component's type parameters the same way `React.memo`
 * does, so the original signature is restored on the way out. The result still
 * suspends while loading and must be rendered under a `Suspense` boundary.
 */
export function genericLazy<TComponent extends GenericComponent>(
  load: () => Promise<{ default: TComponent }>,
): TComponent {
  const loader = load as unknown as () => Promise<{
    default: ComponentType<unknown>;
  }>;
  return lazy(loader) as unknown as TComponent;
}
