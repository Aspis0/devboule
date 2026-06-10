// Reusable React error boundary.
//
// A render/lifecycle throw anywhere below this component is caught here and a
// contained fallback is shown instead of letting the error propagate up and
// blank (white-screen) the whole app. Used to wrap the lazy Polis view so a
// runtime throw inside PixiJS / the renderer degrades to a small "failed to
// load — Retry" card, not a dead app.
//
// NOTE: an error boundary does NOT catch async errors, event-handler errors, or
// a synchronous main-thread FREEZE — only errors thrown during rendering,
// lifecycle methods, and constructors of the tree below it. The Polis freeze is
// addressed separately by the non-blocking chunked build; this is the remaining
// safety net for genuine runtime throws.

import { Component, type ErrorInfo, type ReactNode } from "react";

interface ErrorBoundaryProps {
  children: ReactNode;
  /** Optional custom fallback. Receives the caught error and a `reset` fn that
   *  clears the error state so the children re-mount and re-render. */
  fallback?: (error: Error, reset: () => void) => ReactNode;
  /** Human label for the default fallback ("<label> failed to load"). */
  label?: string;
}

interface ErrorBoundaryState {
  error: Error | null;
}

export class ErrorBoundary extends Component<
  ErrorBoundaryProps,
  ErrorBoundaryState
> {
  state: ErrorBoundaryState = { error: null };

  static getDerivedStateFromError(error: Error): ErrorBoundaryState {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo): void {
    // Privacy: log only the boundary label + message/stack to the dev console —
    // never user data. This is a local console.error, not telemetry.
    // eslint-disable-next-line no-console
    console.error(
      `[ErrorBoundary${this.props.label ? ` ${this.props.label}` : ""}]`,
      error,
      info.componentStack,
    );
  }

  private reset = (): void => {
    this.setState({ error: null });
  };

  render(): ReactNode {
    const { error } = this.state;
    if (error) {
      if (this.props.fallback) return this.props.fallback(error, this.reset);
      const label = this.props.label ?? "This view";
      return (
        <div className="flex h-full min-h-[240px] items-center justify-center p-6">
          <div className="max-w-[440px] rounded-2xl border border-coral/20 bg-white px-5 py-4 text-center shadow-sm">
            <p className="text-[13px] font-semibold text-coral-dark">
              {label} failed to load
            </p>
            <p className="mt-1 break-words text-[12px] text-cream-500">
              {error.message || "An unexpected error occurred."}
            </p>
            <button
              onClick={this.reset}
              className="mt-3 rounded-xl bg-terracotta px-3 py-1.5 text-[12px] font-medium text-white hover:bg-terracotta-500"
            >
              Retry
            </button>
          </div>
        </div>
      );
    }
    return this.props.children;
  }
}

export default ErrorBoundary;
