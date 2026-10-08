import { invokeTauri } from "@/shared/api/tauri";

/** A layer 0 goal: private to its owner, with an optional shared part. */
export type Layer0Goal = {
  private: string;
  public: string;
  publicEnabled: boolean;
};

type RawLayer0Goal = {
  private: string;
  public: string;
  public_enabled: boolean;
};

/** `agentPubkey` selects one of your agents; `null` is your own goal. */
export async function getLayer0Goal(
  agentPubkey: string | null,
): Promise<Layer0Goal> {
  const raw = await invokeTauri<RawLayer0Goal>("get_layer0_goal", {
    agentPubkey,
  });
  return {
    private: raw.private,
    public: raw.public,
    publicEnabled: raw.public_enabled,
  };
}

export async function setLayer0Goal(
  agentPubkey: string | null,
  goal: Layer0Goal,
): Promise<void> {
  await invokeTauri("set_layer0_goal", {
    agentPubkey,
    goal: {
      private: goal.private,
      public: goal.public,
      public_enabled: goal.publicEnabled,
    },
  });
}

export async function getPublicGoal(pubkey: string): Promise<string | null> {
  return invokeTauri<string | null>("get_public_goal", { pubkey });
}

/** Mirror the "Goal layers" experiment toggle for agent spawns. */
export async function setGoalsFeatureEnabled(enabled: boolean): Promise<void> {
  await invokeTauri<void>("set_goals_feature_enabled", { enabled });
}
