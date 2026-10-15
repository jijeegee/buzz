import type {
  MentionPubkeyCandidate,
  RepeatedMention,
} from "./extractMentionPubkeys";
import * as React from "react";

import type { DraftMentionRef } from "./useDrafts";

import { trimMapToSize } from "@/shared/lib/trimMapToSize";
import {
  replaceWithDraftMentionRefs,
  snapshotDraftMentionRefs,
} from "./draftMentionRefs";

export function useDraftMentionRouting(params: {
  memberCandidates?: readonly MentionPubkeyCandidate[];
  mentionMapRef: React.MutableRefObject<Map<string, string>>;
  personaMentionMapRef: React.MutableRefObject<Map<string, string>>;
  repeatedMentionsRef?: React.MutableRefObject<RepeatedMention[]>;
  selectedAgentPubkeysRef: React.MutableRefObject<Set<string>>;
  selectedAgentNamesRef: React.MutableRefObject<string[]>;
  cancelAutocomplete: () => void;
  setSelectedNames: (names: string[]) => void;
  setSelectedAgentNames: (names: string[]) => void;
}): {
  getDraftMentionRefs: (
    content: string,
    fallbackRefs?: readonly DraftMentionRef[],
    competingDisplayNames?: readonly string[],
  ) => DraftMentionRef[];
  restoreDraftMentionRefs: (refs: readonly DraftMentionRef[]) => void;
} {
  const getDraftMentionRefs = React.useCallback(
    (
      content: string,
      fallbackRefs: readonly DraftMentionRef[] = [],
      competingDisplayNames: readonly string[] = [],
    ) =>
      snapshotDraftMentionRefs(
        content,
        params.mentionMapRef.current,
        params.selectedAgentNamesRef.current,
        params.memberCandidates,
        params.personaMentionMapRef.current.keys(),
        fallbackRefs,
        competingDisplayNames,
        params.repeatedMentionsRef?.current,
      ),
    [
      params.mentionMapRef,
      params.repeatedMentionsRef,
      params.selectedAgentNamesRef,
      params.memberCandidates,
      params.personaMentionMapRef,
    ],
  );
  const restoreDraftMentionRefs = React.useCallback(
    (refs: readonly DraftMentionRef[]) => {
      params.cancelAutocomplete();
      params.selectedAgentPubkeysRef.current = new Set(
        refs
          .filter((ref) => ref.isAgent)
          .map((ref) => ref.pubkey.toLowerCase()),
      );
      const { names, agentNames } = replaceWithDraftMentionRefs(
        refs,
        params.mentionMapRef.current,
        params.personaMentionMapRef.current,
        params.repeatedMentionsRef?.current,
      );
      trimMapToSize(params.mentionMapRef.current, 200);
      params.selectedAgentNamesRef.current = agentNames;
      params.setSelectedNames(names);
      params.setSelectedAgentNames(agentNames);
    },
    [params],
  );
  return { getDraftMentionRefs, restoreDraftMentionRefs };
}
