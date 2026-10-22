import { useMutation, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";

import {
  SettingsOptionGroup,
  SettingsOptionRow,
} from "@/features/settings/ui/SettingsOptionGroup";
import { useIdentityQuery } from "@/shared/api/hooks";
import { useT } from "@/shared/i18n";
import { SegmentedControl } from "@/shared/ui/segmented-control";
import {
  type GroupAddPolicy,
  groupsErrorText,
  setGroupAddPolicy,
} from "../groupsApi";
import { groupAddPolicyQueryKey, useGroupAddPolicyQuery } from "../hooks";

/**
 * Settings › Privacy: who can add me to groups. With Friends (the default),
 * anyone else sends an invitation instead.
 */
export function GroupAddPolicySetting() {
  const t = useT();
  const options = [
    { value: "friends", label: t("groups.addPolicy.friends") },
    { value: "everyone", label: t("groups.addPolicy.everyone") },
  ] as const;
  const queryClient = useQueryClient();
  const account = useIdentityQuery().data?.pubkey?.toLowerCase() ?? "";
  const queryKey = groupAddPolicyQueryKey(account);
  const policyQuery = useGroupAddPolicyQuery();
  const save = useMutation({
    mutationFn: (next: GroupAddPolicy) => setGroupAddPolicy(next),
    onMutate: (next) => {
      const previous = queryClient.getQueryData<GroupAddPolicy>(queryKey);
      queryClient.setQueryData<GroupAddPolicy>(queryKey, next);
      return { previous };
    },
    onError: (error, _next, context) => {
      queryClient.setQueryData(queryKey, context?.previous);
      toast.error(groupsErrorText(error) || t("groups.addPolicy.saveFailed"));
    },
  });

  return (
    <SettingsOptionGroup
      data-testid="settings-group-add-policy"
      title={t("groups.addPolicy.sectionTitle")}
    >
      <SettingsOptionRow>
        <p className="text-sm font-medium">{t("groups.addPolicy.label")}</p>
        <SegmentedControl
          disabled={!policyQuery.isSuccess || save.isPending}
          legend={t("groups.addPolicy.label")}
          onValueChange={(next) => {
            if (next !== policyQuery.data) save.mutate(next);
          }}
          optionTestIdPrefix="settings-group-add-policy"
          options={options}
          size="compact"
          testId="settings-group-add-policy-control"
          value={policyQuery.data ?? "friends"}
        />
      </SettingsOptionRow>
      {policyQuery.isError ? (
        <SettingsOptionRow>
          <p className="text-sm text-muted-foreground">
            {t("groups.addPolicy.loadFailed")}
          </p>
        </SettingsOptionRow>
      ) : null}
    </SettingsOptionGroup>
  );
}
