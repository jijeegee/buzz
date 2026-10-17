import * as React from "react";

/** Back in Kuzz this long with a browser sign-in unfinished: treat it as dropped. */
export const BROWSER_RETURN_GRACE_MS = 4000;

/**
 * A system-browser tab can't be watched, so coming back to Kuzz is the
 * signal: while `active`, leaving the window and returning to it for
 * `BROWSER_RETURN_GRACE_MS` calls `onReturn`. Leaving again resets the wait.
 */
export function useReturnedToApp(active: boolean, onReturn: () => void) {
  const latest = React.useRef(onReturn);
  latest.current = onReturn;

  React.useEffect(() => {
    if (!active) return;
    let left = !document.hasFocus();
    let timer: ReturnType<typeof setTimeout> | undefined;
    const onBlur = () => {
      left = true;
      clearTimeout(timer);
    };
    const onFocus = () => {
      if (!left) return;
      clearTimeout(timer);
      timer = setTimeout(() => latest.current(), BROWSER_RETURN_GRACE_MS);
    };
    window.addEventListener("blur", onBlur);
    window.addEventListener("focus", onFocus);
    return () => {
      clearTimeout(timer);
      window.removeEventListener("blur", onBlur);
      window.removeEventListener("focus", onFocus);
    };
  }, [active]);
}
