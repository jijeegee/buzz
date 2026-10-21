import { Component, type ReactNode } from "react";

type AreaErrorBoundaryProps = {
  /** Area name for the log line, e.g. "sidebar". */
  area: string;
  children: ReactNode;
  className?: string;
  /** Clears a caught error when it changes, e.g. on navigation. */
  resetKey?: unknown;
};

type AreaErrorBoundaryState = {
  error: Error | null;
  resetKey: unknown;
};

/**
 * Render fence for one area of the window (sidebar, main pane, settings).
 *
 * Without it a throw anywhere in the shell reaches the router's default error
 * screen and replaces the whole window. With it only the broken area shows a
 * retry notice; the rest of the app stays usable.
 */
export class AreaErrorBoundary extends Component<
  AreaErrorBoundaryProps,
  AreaErrorBoundaryState
> {
  override state: AreaErrorBoundaryState = {
    error: null,
    resetKey: this.props.resetKey,
  };

  static getDerivedStateFromError(
    error: unknown,
  ): Partial<AreaErrorBoundaryState> {
    return { error: error instanceof Error ? error : new Error(String(error)) };
  }

  static getDerivedStateFromProps(
    props: AreaErrorBoundaryProps,
    state: AreaErrorBoundaryState,
  ): Partial<AreaErrorBoundaryState> | null {
    if (Object.is(props.resetKey, state.resetKey)) return null;
    return { error: null, resetKey: props.resetKey };
  }

  override componentDidCatch(error: unknown, info: React.ErrorInfo): void {
    console.error(
      `[AreaErrorBoundary:${this.props.area}] render error:`,
      error,
      info,
    );
  }

  override render(): ReactNode {
    if (!this.state.error) return this.props.children;
    return (
      <div
        className={
          this.props.className ??
          "flex min-h-0 min-w-0 flex-1 flex-col items-center justify-center gap-3 px-6 text-foreground"
        }
        data-testid={`area-error-${this.props.area}`}
        role="alert"
      >
        <p className="text-center text-sm text-muted-foreground">
          This part of Kuzz couldn't load.
        </p>
        <button
          type="button"
          className="rounded-md border border-border bg-secondary px-3 py-1.5 text-sm hover:bg-secondary/80"
          onClick={() => this.setState({ error: null })}
        >
          Try again
        </button>
      </div>
    );
  }
}
