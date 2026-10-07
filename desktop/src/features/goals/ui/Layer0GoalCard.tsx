import * as React from "react";
import { Target } from "lucide-react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import {
  getLayer0Goal,
  getPublicGoal,
  type Layer0Goal,
  setLayer0Goal,
} from "@/shared/api/tauriLayer0";
import { Button } from "@/shared/ui/button";
import { Switch } from "@/shared/ui/switch";
import { Textarea } from "@/shared/ui/textarea";

const layer0Key = (agentPubkey: string | null) => [
  "layer0-goal",
  agentPubkey ?? "self",
];
const publicGoalKey = (pubkey: string) => ["public-goal", pubkey.toLowerCase()];

/**
 * Edit a layer 0 goal — yours (`agentPubkey` null) or one of your agents'.
 * The private part stays on this device and reaches only your own agents;
 * the public part is off until you turn it on.
 */
export function Layer0GoalCard({
  agentPubkey,
}: {
  agentPubkey: string | null;
}) {
  const queryClient = useQueryClient();
  const goalQuery = useQuery({
    queryKey: layer0Key(agentPubkey),
    queryFn: () => getLayer0Goal(agentPubkey),
  });
  const [draft, setDraft] = React.useState<Layer0Goal | null>(null);
  const goal = draft ?? goalQuery.data ?? null;
  const saveMutation = useMutation({
    mutationFn: (next: Layer0Goal) => setLayer0Goal(agentPubkey, next),
    onSuccess: () => {
      setDraft(null);
      void queryClient.invalidateQueries({ queryKey: layer0Key(agentPubkey) });
      void queryClient.invalidateQueries({ queryKey: ["public-goal"] });
    },
  });
  const privateId = React.useId();
  const publicId = React.useId();

  if (!goal) return null;
  const update = (patch: Partial<Layer0Goal>) =>
    setDraft({ ...goal, ...patch });
  const isAgent = agentPubkey !== null;

  return (
    <section
      className="space-y-3 rounded-2xl border border-border/70 p-4"
      data-testid="layer0-goal-card"
    >
      <div className="flex items-center gap-2">
        <Target className="h-4 w-4 text-primary" />
        <h3 className="text-sm font-semibold">
          {isAgent ? "Agent goal (layer 0)" : "My goal (layer 0)"}
        </h3>
      </div>
      <div className="space-y-1">
        <label
          className="text-xs font-medium text-muted-foreground"
          htmlFor={privateId}
        >
          Private —{" "}
          {isAgent
            ? "only this agent sees it"
            : "only you and your own agents see it"}
        </label>
        <Textarea
          className="min-h-20"
          data-testid="layer0-private"
          id={privateId}
          onChange={(event) => update({ private: event.target.value })}
          value={goal.private}
        />
      </div>
      <div className="flex items-center justify-between gap-3">
        <label className="text-sm" htmlFor={`${publicId}-switch`}>
          Share part of it publicly
        </label>
        <Switch
          checked={goal.publicEnabled}
          data-testid="layer0-public-toggle"
          id={`${publicId}-switch`}
          onCheckedChange={(checked) => update({ publicEnabled: checked })}
        />
      </div>
      {goal.publicEnabled ? (
        <div className="space-y-1">
          <label
            className="text-xs font-medium text-muted-foreground"
            htmlFor={publicId}
          >
            Public — shown on {isAgent ? "this agent's" : "your"} profile
          </label>
          <Textarea
            className="min-h-16"
            data-testid="layer0-public"
            id={publicId}
            onChange={(event) => update({ public: event.target.value })}
            value={goal.public}
          />
        </div>
      ) : null}
      {saveMutation.error instanceof Error ? (
        <p className="text-xs text-destructive">{saveMutation.error.message}</p>
      ) : null}
      <div className="flex items-center justify-between gap-2">
        <p className="text-xs text-muted-foreground">
          Running agents use a changed goal after their next restart.
        </p>
        <Button
          data-testid="layer0-save"
          disabled={draft === null || saveMutation.isPending}
          onClick={() => saveMutation.mutate(goal)}
          size="sm"
          type="button"
        >
          {saveMutation.isPending ? "Saving…" : "Save"}
        </Button>
      </div>
    </section>
  );
}

/** Someone's public layer 0 goal, when they share one. */
export function PublicGoalLine({ pubkey }: { pubkey: string }) {
  const goalQuery = useQuery({
    queryKey: publicGoalKey(pubkey),
    queryFn: () => getPublicGoal(pubkey),
    staleTime: 60_000,
  });
  if (!goalQuery.data) return null;
  return (
    <div
      className="flex items-start gap-2 rounded-xl bg-muted/40 px-3 py-2 text-sm"
      data-testid="public-goal"
    >
      <Target className="mt-0.5 h-4 w-4 shrink-0 text-primary" />
      <p className="whitespace-pre-wrap break-words">{goalQuery.data}</p>
    </div>
  );
}
