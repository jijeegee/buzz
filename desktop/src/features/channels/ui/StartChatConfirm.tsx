import * as React from "react";

import { useUsersBatchQuery } from "@/features/profile/hooks";
import { resolveUserLabel } from "@/features/profile/lib/identity";
import {
  AlertDialog,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/shared/ui/alert-dialog";
import { Button } from "@/shared/ui/button";

import { formatChatMemberTitle } from "../lib/groupChat";
import { t as translate, useT } from "@/shared/i18n";

type Pending = {
  pubkeys: string[];
  resolve: (confirmed: boolean) => void;
};

/** "Ann", "Ann and Bob", "Ann, Bob and Cy", "Ann, Bob, Cy +2". */
export function joinPeopleNames(names: readonly string[]): string {
  if (names.length <= 1) return names[0] ?? "";
  if (names.length <= 3) {
    return translate("channels.startChat.nameList", {
      names: names.slice(0, -1).join(", "),
      last: names[names.length - 1],
    });
  }
  return formatChatMemberTitle(names);
}

/** Copy for the prompt shown before a mention starts a new chat. */
export function startChatPromptText(names: readonly string[]) {
  const who = joinPeopleNames(names);
  return {
    title: translate("channels.startChat.title", { who }),
    description: translate("channels.startChat.body", {
      count: names.length,
      who,
    }),
  };
}

/**
 * Asks before a mention in a 1:1 (or a new message) turns into a new chat.
 * `confirm(pubkeys)` resolves true only when the person agrees; render
 * `dialog` once next to the composer.
 */
export function useStartChatConfirm() {
  const [pending, setPending] = React.useState<Pending | null>(null);
  const pendingRef = React.useRef<Pending | null>(null);

  const settle = React.useCallback((confirmed: boolean) => {
    const current = pendingRef.current;
    pendingRef.current = null;
    setPending(null);
    current?.resolve(confirmed);
  }, []);

  React.useEffect(() => () => pendingRef.current?.resolve(false), []);

  const confirm = React.useCallback(
    (pubkeys: string[]) =>
      new Promise<boolean>((resolve) => {
        pendingRef.current?.resolve(false);
        const next = { pubkeys, resolve };
        pendingRef.current = next;
        setPending(next);
      }),
    [],
  );

  const dialog = (
    <StartChatConfirmDialog
      onCancel={() => settle(false)}
      onConfirm={() => settle(true)}
      pubkeys={pending?.pubkeys ?? null}
    />
  );
  return { confirm, dialog };
}

function StartChatConfirmDialog({
  onCancel,
  onConfirm,
  pubkeys,
}: {
  onCancel: () => void;
  onConfirm: () => void;
  pubkeys: string[] | null;
}) {
  const t = useT();
  const profiles = useUsersBatchQuery(pubkeys ?? [], {
    enabled: (pubkeys?.length ?? 0) > 0,
  }).data?.profiles;
  const names = (pubkeys ?? []).map((pubkey) =>
    resolveUserLabel({ currentPubkey: undefined, profiles, pubkey }),
  );
  const text = startChatPromptText(names);
  return (
    <AlertDialog
      onOpenChange={(open) => {
        if (!open) onCancel();
      }}
      open={pubkeys !== null}
    >
      <AlertDialogContent data-testid="start-chat-confirm">
        <AlertDialogHeader>
          <AlertDialogTitle>{text.title}</AlertDialogTitle>
          <AlertDialogDescription>{text.description}</AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <Button onClick={onCancel} size="sm" type="button" variant="outline">
            {t("common.cancel")}
          </Button>
          <Button
            data-testid="start-chat-confirm-accept"
            onClick={onConfirm}
            size="sm"
            type="button"
          >
            {t("channels.startChat.confirm")}
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
