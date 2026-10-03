import * as React from "react";

import { useManagedAgentsQuery } from "@/features/agents/hooks";
import { findDefaultAi } from "@/features/agents/lib/defaultAi";

/**
 * The managed agent starred as this desktop's default AI, read from the
 * shared managed-agents query. `defaultAi` is `null` both while the list is
 * still loading and when no agent is starred; `isLoading` tells them apart.
 */
export function useDefaultAi() {
  const managedAgentsQuery = useManagedAgentsQuery();
  const agents = managedAgentsQuery.data;
  const defaultAi = React.useMemo(
    () => (agents ? findDefaultAi(agents) : null),
    [agents],
  );
  return { defaultAi, isLoading: managedAgentsQuery.isPending };
}
