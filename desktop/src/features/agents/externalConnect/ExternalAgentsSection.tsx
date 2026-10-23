import * as React from "react";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";

import { useUserProfileQuery } from "@/features/profile/hooks";
import { connectHermes } from "@/shared/api/agentPairing";
import { revokeAuthDevice } from "@/shared/api/tokenAuth";
import { useT } from "@/shared/i18n";
import { ProfileAvatar } from "@/features/profile/ui/ProfileAvatar";
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/shared/ui/alert-dialog";
import { Button } from "@/shared/ui/button";
import {
  AGENT_TILE_GRID_CLASS,
  AgentTile,
  DeviceHeading,
} from "../ui/AgentTile";
import type { ExternalAgent, ExternalAgentGroup } from "./pairingLogic";
import {
  formatConnectedAt,
  refreshExternalAgents,
  useExternalAgentGroups,
  useExternalAgentName,
  useExternalConnectAvailable,
  useHermesStatusQuery,
} from "./useExternalAgents";

/**
 * My agents: the account's paired computers (`kuzz pair`), each with the
 * agents connected on it and a Disconnect that signs that computer out
 * after a confirm. Renders nothing when there are none.
 */
export function ExternalAgentsSection({
  robot,
}: {
  /** The device robot for a computer's heading. */
  robot: (deviceId: string) => React.ReactNode;
}) {
  const t = useT();
  const queryClient = useQueryClient();
  const { groups, hermesPubkeys } = useExternalAgentGroups();
  const [confirming, setConfirming] = React.useState<ExternalAgentGroup | null>(
    null,
  );
  const [busy, setBusy] = React.useState(false);

  async function disconnect(group: ExternalAgentGroup) {
    setBusy(true);
    try {
      await revokeAuthDevice(group.deviceId);
      toast.success(
        t("agents.external.disconnected", { device: group.deviceName }),
      );
    } catch (error) {
      toast.error(
        error instanceof Error
          ? error.message
          : t("agents.external.disconnectFailed"),
      );
    } finally {
      setBusy(false);
      refreshExternalAgents(queryClient);
    }
  }

  if (groups.length === 0) return null;

  return (
    <>
      {groups.map((group) => (
        <div
          className="space-y-3"
          data-testid={`agents-external-device-${group.deviceId}`}
          key={group.deviceId}
        >
          <DeviceHeading
            detail={t("agents.external.agentCount", {
              count: group.agents.length,
            })}
            robot={robot(group.deviceId)}
            title={group.deviceName}
          />
          <div className={AGENT_TILE_GRID_CLASS}>
            {(group.agents.length > 0 ? group.agents : [null]).map((agent) => (
              <ExternalAgentTile
                agent={agent}
                busy={busy}
                deviceName={group.deviceName}
                hermesPubkeys={hermesPubkeys}
                key={agent?.pubkey ?? "unknown"}
                onDisconnect={() => setConfirming(group)}
              />
            ))}
          </div>
        </div>
      ))}
      <AlertDialog
        onOpenChange={(open) => {
          if (!open) setConfirming(null);
        }}
        open={confirming !== null}
      >
        <AlertDialogContent data-testid="external-agent-disconnect-dialog">
          <AlertDialogHeader>
            <AlertDialogTitle>
              {t("agents.external.disconnectTitle", {
                device: confirming?.deviceName ?? "",
              })}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {t("agents.external.disconnectDescription", {
                device: confirming?.deviceName ?? "",
              })}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{t("common.cancel")}</AlertDialogCancel>
            <Button
              data-testid="external-agent-disconnect-confirm"
              onClick={() => {
                const group = confirming;
                setConfirming(null);
                if (group) void disconnect(group);
              }}
              type="button"
              variant="destructive"
            >
              {t("agents.external.disconnect")}
            </Button>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}

function ExternalAgentTile({
  agent,
  busy,
  deviceName,
  hermesPubkeys,
  onDisconnect,
}: {
  /** `null`: a paired computer whose agent this app has not seen. */
  agent: ExternalAgent | null;
  busy: boolean;
  deviceName: string;
  hermesPubkeys: ReadonlySet<string>;
  onDisconnect: () => void;
}) {
  const t = useT();
  const pubkey = agent?.pubkey ?? null;
  const name = useExternalAgentName(pubkey, hermesPubkeys);
  const profile = useUserProfileQuery(pubkey ?? undefined).data;
  const at = formatConnectedAt(agent?.connectedAt ?? null);
  return (
    <AgentTile
      avatarUrl={profile?.avatarUrl}
      buttons={
        <Button
          className="text-destructive hover:text-destructive"
          data-testid={`external-agent-disconnect-${pubkey ?? deviceName}`}
          disabled={busy}
          onClick={onDisconnect}
          size="xs"
          variant="outline"
        >
          {t("agents.external.disconnect")}
        </Button>
      }
      dataTestId={`external-agent-${pubkey ?? deviceName}`}
      description={at ? t("agents.external.connectedOn", { time: at }) : null}
      label={name}
      openLabel={t("agents.external.disconnectNamed", { name })}
      subtitle={deviceName}
      onOpen={onDisconnect}
    />
  );
}

/**
 * "Hermes found on this computer": shown when Hermes is installed here
 * (`$HERMES_HOME` or `%LOCALAPPDATA%\hermes`) and no Hermes agent of this
 * account is connected; Connect pairs it in one step.
 */
export function HermesFoundCard() {
  const t = useT();
  const queryClient = useQueryClient();
  const available = useExternalConnectAvailable();
  const hermes = useHermesStatusQuery(available);
  const { groups } = useExternalAgentGroups();
  const [connecting, setConnecting] = React.useState(false);

  const liveDevices = new Set(groups.map((group) => group.deviceId));
  const connected = (hermes.data?.agents ?? []).some(
    (agent) => agent.deviceId !== null && liveDevices.has(agent.deviceId),
  );
  if (!available || !hermes.data?.found || connected) return null;

  async function connect() {
    setConnecting(true);
    try {
      const result = await connectHermes();
      toast.success(
        t("agents.external.connectedToast", {
          agent: "Hermes",
          device: result.deviceName ?? t("agents.external.thisComputer"),
        }),
      );
    } catch (error) {
      toast.error(
        error instanceof Error
          ? error.message
          : t("agents.external.hermesFailed"),
      );
    } finally {
      setConnecting(false);
      refreshExternalAgents(queryClient);
    }
  }

  return (
    <div
      className="flex min-w-0 items-center gap-3 rounded-xl border border-border/70 bg-muted/30 px-4 py-3"
      data-testid="hermes-found-card"
    >
      <ProfileAvatar
        avatarUrl={null}
        className="h-9 w-9 shrink-0 text-xs shadow-none"
        iconClassName="h-4 w-4"
        label="Hermes"
        mediaVariant="small"
        shape="circle"
      />
      <p className="min-w-0 flex-1 truncate text-sm font-medium">
        {t("agents.external.hermesFound")}
      </p>
      <Button
        data-testid="hermes-connect"
        disabled={connecting}
        onClick={() => void connect()}
        size="sm"
        type="button"
      >
        {connecting
          ? t("agents.external.hermesConnecting")
          : t("agents.external.hermesConnect")}
      </Button>
    </div>
  );
}
