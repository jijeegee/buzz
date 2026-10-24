import type { SignInProvider } from "@/shared/api/tokenAuth";
import { useT } from "@/shared/i18n";
import { Button } from "@/shared/ui/button";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/shared/ui/dialog";

/**
 * "Add account": pick how the new account signs in. Rendered by the profile
 * card and Settings › Account, outside the profile menu, so it survives the
 * menu closing. Choosing closes it and starts that sign-in.
 */
export function AddAccountDialog({
  open,
  onOpenChange,
  onChoose,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onChoose: (provider: SignInProvider) => void;
}) {
  const t = useT();
  const choose = (provider: SignInProvider) => {
    onOpenChange(false);
    onChoose(provider);
  };
  return (
    <Dialog onOpenChange={onOpenChange} open={open}>
      <DialogContent
        aria-describedby={undefined}
        className="max-w-sm"
        data-testid="add-account-dialog"
      >
        <DialogHeader>
          <DialogTitle>{t("accounts.add.title")}</DialogTitle>
        </DialogHeader>
        <div className="space-y-2">
          <Button
            className="h-10 w-full"
            data-testid="add-account-google"
            onClick={() => choose("google")}
            type="button"
            variant="outline"
          >
            {t("accounts.add.google")}
          </Button>
          <Button
            className="h-10 w-full"
            data-testid="add-account-apple"
            onClick={() => choose("apple")}
            type="button"
            variant="outline"
          >
            {t("accounts.add.apple")}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
