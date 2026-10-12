import { Toaster as Sonner, toast } from "sonner";

import { toUserFacingMessage } from "@/shared/lib/userFacingError";
import { useTheme } from "@/shared/theme/ThemeProvider";

// Error and warning toasts often show raw backend text, which can name
// protocol internals (relay, npub, NIP-xx…). Sonner's `toast` is a shared
// mutable object, so wrapping these two methods once here maps every call
// site's string title and description to plain wording. Guarded so a second
// evaluation (HMR) doesn't wrap twice.
const USER_FACING_WRAPPED = Symbol.for("kuzz.toast.userFacingWrapped");

type ToastMethod = typeof toast.error;

function wrapUserFacing(method: ToastMethod): ToastMethod {
  return (message, data) =>
    method(
      typeof message === "string" ? toUserFacingMessage(message) : message,
      data && typeof data.description === "string"
        ? { ...data, description: toUserFacingMessage(data.description) }
        : data,
    );
}

const toastFlags = toast as unknown as Record<symbol, boolean>;
if (!toastFlags[USER_FACING_WRAPPED]) {
  toast.error = wrapUserFacing(toast.error);
  toast.warning = wrapUserFacing(toast.warning);
  toastFlags[USER_FACING_WRAPPED] = true;
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
