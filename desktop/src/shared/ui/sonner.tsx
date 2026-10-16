import { Toaster as Sonner, toast } from "sonner";

import { mapBackendErrorInText } from "@/shared/lib/userFacingError";
import { useTheme } from "@/shared/theme/ThemeProvider";

// Many error and warning toasts show raw backend text. Sonner's `toast` is a
// shared mutable object, so wrapping these two methods once here maps only the
// known raw backend error shapes (see `mapBackendErrorInText`) in every call
// site's string title and description; the caller's own wording and user
// content pass through. Guarded so a second evaluation (HMR) doesn't wrap
// twice.
const BACKEND_ERROR_WRAPPED = Symbol.for("kuzz.toast.backendErrorWrapped");

type ToastMethod = typeof toast.error;

function wrapBackendErrors(method: ToastMethod): ToastMethod {
  return (message, data) =>
    method(
      typeof message === "string" ? mapBackendErrorInText(message) : message,
      data && typeof data.description === "string"
        ? { ...data, description: mapBackendErrorInText(data.description) }
        : data,
    );
}

const toastFlags = toast as unknown as Record<symbol, boolean>;
if (!toastFlags[BACKEND_ERROR_WRAPPED]) {
  toast.error = wrapBackendErrors(toast.error);
  toast.warning = wrapBackendErrors(toast.warning);
  toastFlags[BACKEND_ERROR_WRAPPED] = true;
}

type ToasterProps = React.ComponentProps<typeof Sonner>;

const Toaster = ({ ...props }: ToasterProps) => {
  const { isDark } = useTheme();

  return (
    <Sonner
      theme={isDark ? "dark" : "light"}
      className="toaster group"
      toastOptions={{
        classNames: {
          toast:
            "group toast group-[.toaster]:bg-background group-[.toaster]:text-foreground group-[.toaster]:border-border group-[.toaster]:shadow-lg",
          description: "group-[.toast]:text-muted-foreground",
          actionButton:
            "group-[.toast]:bg-primary group-[.toast]:text-primary-foreground",
          cancelButton:
            "group-[.toast]:bg-muted group-[.toast]:text-muted-foreground",
        },
      }}
      {...props}
    />
  );
};

export { Toaster };
