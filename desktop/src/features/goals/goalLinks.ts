import {
  getChannelIdFromTags,
  getThreadReference,
} from "@/features/messages/lib/threading";
import { getEventsByIds, getThreadReplies } from "@/shared/api/tauri";
import type { RelayEvent } from "@/shared/api/types";
import type { GoalLinkInfo } from "./goalTree";

type GoalLinkDeps = {
  getEventsByIds: (ids: string[]) => Promise<RelayEvent[]>;
  hasReplies: (rootId: string, channelId: string) => Promise<boolean>;
};

const defaultDeps: GoalLinkDeps = {
  getEventsByIds,
  hasReplies: async (rootId, channelId) =>
    (await getThreadReplies(rootId, channelId, { limit: 1, depthLimit: 1 }))
      .events.length > 0,
};

/**
 * Classify a goal's linked ids (see `GoalLinkInfo`) by reading each event:
 * a reply names its thread root; a top-level message is a thread once it
 * has a reply. Events that are gone or belong to another channel are
 * `missing`.
 */
export async function classifyGoalLinks(
  channelId: string,
  ids: readonly string[],
  deps: GoalLinkDeps = defaultDeps,
): Promise<Map<string, GoalLinkInfo>> {
  const events = await deps.getEventsByIds([...ids]);
  const byId = new Map(events.map((event) => [event.id.toLowerCase(), event]));
  const entries = await Promise.all(
    ids.map(async (id): Promise<[string, GoalLinkInfo]> => {
      const event = byId.get(id.toLowerCase());
      if (!event || getChannelIdFromTags(event.tags) !== channelId) {
        return [id, { kind: "missing" }];
      }
      const { rootId } = getThreadReference(event.tags);
      if (rootId) return [id, { kind: "reply", threadRootId: rootId }];
      const replied = await deps.hasReplies(event.id, channelId);
      return [id, { kind: replied ? "thread" : "message" }];
    }),
  );
  return new Map(entries);
}
