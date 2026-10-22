import * as React from "react";
import { Target } from "lucide-react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import {
  getLayer0Goal,
  getPublicGoal,
  type Layer0Goal,
  setLayer0Goal,
} from "@/shared/api/tauriLayer0";
import { useT } from "@/shared/i18n";
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
  const t = useT();
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
      <div className="space-y-1">
        <div className="flex items-center gap-2">
          <Target className="h-4 w-4 text-primary" />
          <h3 className="text-sm font-semibold">
            {isAgent ? t("goals.layer0.agentTitle") : t("goals.layer0.myTitle")}
          </h3>
        </div>
        <p
          className="text-xs text-muted-foreground/70"
          data-testid="layer0-goal-description"
        >
          {isAgent
            ? t("goals.layer0.agentDescription")
            : t("goals.layer0.myDescription")}
        </p>
      </div>
      <div className="space-y-1">
        <label
          className="text-xs font-medium text-muted-foreground"
          htmlFor={privateId}
        >
          {isAgent
            ? t("goals.layer0.agentPrivateLabel")
            : t("goals.layer0.myPrivateLabel")}
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
          {t("goals.layer0.sharePublicly")}
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
            {isAgent
              ? t("goals.layer0.agentPublicLabel")
              : t("goals.layer0.myPublicLabel")}
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
          {t("goals.layer0.appliesNextMessage")}
        </p>
        <Button
          data-testid="layer0-save"
          disabled={draft === null || saveMutation.isPending}
          onClick={() => saveMutation.mutate(goal)}
          size="sm"
          type="button"
        >
          {saveMutation.isPending ? t("goals.editor.saving") : t("common.save")}
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
