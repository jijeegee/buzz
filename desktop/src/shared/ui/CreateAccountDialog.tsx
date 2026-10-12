import type { useGoogleLogin } from "@/shared/hooks/useGoogleLogin";
import { Button } from "./button";
import {
  AlertDialog,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "./alert-dialog";

/**
 * "Create your Buzz account": the Google account just chosen has no Buzz
 * account yet. Nothing exists until "Create account"; "Use a different
 * account" opens Google's account picker again, and closing stops here.
 */
export function CreateAccountDialog({
  login,
}: {
  login: Pick<ReturnType<typeof useGoogleLogin>, "signup" | "answerSignup">;
}) {
  const prompt = login.signup;
  return (
    <AlertDialog
      open={prompt !== null}
      onOpenChange={(open) => {
        if (!open) login.answerSignup("close");
      }}
    >
      <AlertDialogContent data-testid="create-account-dialog">
        <AlertDialogHeader>
          <AlertDialogTitle>Create your Kuzz account</AlertDialogTitle>
          <AlertDialogDescription>
            There's no Kuzz account for this Google account yet.
          </AlertDialogDescription>
        </AlertDialogHeader>
        {prompt?.email ? (
          <p
            className="truncate text-sm font-medium"
            data-testid="create-account-email"
          >
            {prompt.email}
          </p>
        ) : null}
        <AlertDialogFooter>
          <Button
            data-testid="create-account-different"
            onClick={() => login.answerSignup("different")}
            type="button"
            variant="outline"
          >
            Use a different account
          </Button>
          <Button
            data-testid="create-account-confirm"
            onClick={() => login.answerSignup("create")}
            type="button"
          >
            Create account
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
