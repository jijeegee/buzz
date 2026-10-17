import * as React from "react";
import { toast } from "sonner";

import { useAppNavigation } from "@/app/navigation/useAppNavigation";
import { useOpenDmMutation } from "@/features/channels/hooks";

/** Opens (or creates) the DM with these people, like the profile's Message. */
export function useOpenDm() {
  const openDmMutation = useOpenDmMutation();
  const { goChannel } = useAppNavigation();
  return React.useCallback(
    async (pubkeys: string[]) => {
      const dm = await openDmMutation.mutateAsync({ pubkeys });
      await goChannel(dm.id);
    },
    [goChannel, openDmMutation],
  );
}

/** Message one person; failures show a toast. */
export function useMessagePerson() {
  const openDm = useOpenDm();
  return React.useCallback(
    (pubkey: string) => {
      openDm([pubkey]).catch((error: unknown) =>
        toast.error(
          error instanceof Error ? error.message : "Could not open the chat",
        ),
      );
    },
    [openDm],
  );
}
