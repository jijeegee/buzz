import * as React from "react";
import { listen } from "@tauri-apps/api/event";
import {
  cancelGoogleLogin,
  cancelSignup,
  confirmSignup,
  loginWithGoogle,
  type SignupPrompt,
  type TokenAuthStatus,
} from "@/shared/api/tokenAuth";

type Phase = "opening" | "waiting" | "completing" | "signup" | null;

/** The user's answer on the "Create your Buzz account" screen. */
export type SignupChoice = "create" | "different" | "close";

/** Owns one native login attempt, including its browser-only cancellation. */
export function useGoogleLogin() {
  const [phase, setPhase] = React.useState<Phase>(null);
  const [error, setError] = React.useState<string | null>(null);
  const current = React.useRef<string | null>(null);
  const mounted = React.useRef(true);
  const [signup, setSignup] = React.useState<SignupPrompt | null>(null);
  const answer = React.useRef<((choice: SignupChoice) => void) | null>(null);
  React.useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      answer.current?.("close");
      const id = current.current;
      current.current = null;
      if (id) void cancelGoogleLogin(id).catch(() => {});
    };
  }, []);

  /** Show the new-account screen and wait for the user's choice. */
  function ask(prompt: SignupPrompt): Promise<SignupChoice> {
    return new Promise((resolve) => {
      answer.current = (choice) => {
        answer.current = null;
        setSignup(null);
        resolve(choice);
      };
      setSignup(prompt);
      setPhase("signup");
    });
  }

  /**
   * A sign-in whose Google account has no Buzz account yet: create it, pick
   * another Google account (a fresh sign-in), or stop. Until "Create
   * account" the server holds nothing for it.
   */
  async function settleSignup(
    first: TokenAuthStatus,
    again: () => Promise<TokenAuthStatus>,
  ): Promise<TokenAuthStatus | null> {
    let result = first;
    while (result.signup) {
      const choice = mounted.current ? await ask(result.signup) : "close";
      if (choice === "create") {
        setPhase("completing");
        result = await confirmSignup();
        continue;
      }
      await cancelSignup().catch(() => {});
      if (choice !== "different" || !mounted.current) return null;
      setPhase("opening");
      result = await again();
    }
    return result;
  }

  async function run(
    allowRestore = false,
    existingTokenAccount = false,
    connect = false,
  ) {
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
      const signIn = () =>
        loginWithGoogle(allowRestore, existingTokenAccount, id, connect);
      const result = await settleSignup(await signIn(), signIn);
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

  /** Answer the "Create your Buzz account" screen. */
  function answerSignup(choice: SignupChoice) {
    answer.current?.(choice);
  }

  return {
    run,
    cancel,
    phase,
    error,
    pending: phase !== null,
    signup,
    answerSignup,
  };
}
