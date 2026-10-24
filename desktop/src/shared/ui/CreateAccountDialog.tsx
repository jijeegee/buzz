import { useT } from "@/shared/i18n";
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
 * "Create your Kuzz account": the Google account or Apple ID just chosen has
 * no Kuzz account yet (the text names which). Nothing exists until "Create
 * account"; "Use a different account" signs in the same way again, and
 * closing stops here.
 */
export function CreateAccountDialog({
  login,
}: {
  login: Pick<ReturnType<typeof useGoogleLogin>, "signup" | "answerSignup">;
}) {
  const prompt = login.signup;
  const t = useT();
  return (
    <AlertDialog
      open={prompt !== null}
      onOpenChange={(open) => {
        if (!open) login.answerSignup("close");
      }}
    >
      <AlertDialogContent data-testid="create-account-dialog">
        <AlertDialogHeader>
          <AlertDialogTitle>{t("ui.createAccount.title")}</AlertDialogTitle>
          <AlertDialogDescription>
            {prompt?.provider === "apple"
              ? t("ui.createAccount.descriptionApple")
              : t("ui.createAccount.description")}
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
            {t("ui.createAccount.useDifferent")}
          </Button>
          <Button
            data-testid="create-account-confirm"
            onClick={() => login.answerSignup("create")}
            type="button"
          >
            {t("ui.createAccount.create")}
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
