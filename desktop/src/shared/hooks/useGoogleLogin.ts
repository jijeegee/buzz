import * as React from "react";
import { listen } from "@tauri-apps/api/event";
import { cancelGoogleLogin, loginWithGoogle } from "@/shared/api/tokenAuth";

type Phase = "opening" | "waiting" | "completing" | null;

/** Owns one native login attempt, including its browser-only cancellation. */
export function useGoogleLogin() {
  const [phase, setPhase] = React.useState<Phase>(null);
  const [error, setError] = React.useState<string | null>(null);
  const current = React.useRef<string | null>(null);
  const mounted = React.useRef(true);
  React.useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      const id = current.current;
      current.current = null;
      if (id) void cancelGoogleLogin(id).catch(() => {});
    };
  }, []);

  async function run(allowRestore = false, existingTokenAccount = false) {
    if (current.current) return null;
    const id = crypto.randomUUID();
    current.current = id;
    setError(null);
    setPhase("opening");
    let unlisten: (() => void) | undefined;
    try {
      unlisten = await listen<{
        attemptId: string;
        phase: "waiting" | "completing";
      }>("google-login-progress", ({ payload }) => {
        if (
          payload.attemptId === id &&
          payload.phase === "waiting" &&
          current.current !== id
        ) {
          // Cleanup can run before the async native command claims its slot.
          void cancelGoogleLogin(id).catch(() => {});
        }
        if (
          mounted.current &&
          current.current === id &&
          payload.attemptId === id
        ) {
          setPhase(payload.phase);
        }
      });
      if (!mounted.current || current.current !== id) return null;
      const result = await loginWithGoogle(
        allowRestore,
        existingTokenAccount,
        id,
      );
      return mounted.current && current.current === id ? result : null;
    } catch (cause) {
      const message = cause instanceof Error ? cause.message : String(cause);
      if (
        mounted.current &&
        current.current === id &&
        message !== "Google sign-in cancelled."
      ) {
        setError(message || "Google sign-in failed. Try again.");
      }
      return null;
    } finally {
      unlisten?.();
      if (current.current === id) {
        current.current = null;
        if (mounted.current) setPhase(null);
      }
    }
  }

  async function cancel() {
    const id = current.current;
    if (!id || phase !== "waiting") return;
    try {
      const cancelled = await cancelGoogleLogin(id);
      // Native completion won the race. Let it finish; never sign out.
      if (!cancelled && mounted.current && current.current === id)
        setPhase("completing");
    } catch {
      if (mounted.current && current.current === id) {
        setError("Could not cancel sign-in. Try again.");
      }
    }
  }

  return { run, cancel, phase, error, pending: phase !== null };
}
