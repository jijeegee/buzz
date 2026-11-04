import * as React from "react";
import { useQueryClient } from "@tanstack/react-query";
import { Pencil } from "lucide-react";

import { useIdentityQuery } from "@/shared/api/hooks";
import {
  agentRuntime,
  EXTERNAL_PLATFORM,
} from "@/features/agents/externalConnect/pairingLogic";
import { useExternalAgentName } from "@/features/agents/externalConnect/useExternalAgents";
import { normalizeDeviceId } from "@/shared/api/ownerDevices";
import {
  type AuthDevice,
  type AuthDeviceAgent,
  renameAuthDevice,
} from "@/shared/api/tokenAuth";
import { type MessageKey, useT } from "@/shared/i18n";
import { applyAuthDeviceRename } from "@/shared/api/useAuthDeviceNames";
import {
  publishDeviceRobot,
  useOwnerDevices,
} from "@/shared/api/useOwnerDevices";
import { resolveDeviceRobotVariant } from "@/shared/lib/deviceRobot";
import { Button } from "@/shared/ui/button";
import { Input } from "@/shared/ui/input";

import { DeviceRobotPicker } from "./DeviceRobotPicker";

const DEVICE_NAME_MAX = 64;

/**
 * One signed-in device: its robot (the owner can pick another), a renamable
 * name, and (for other devices) a remote sign-out. A computer paired for an
 * external agent reads "Disconnect" instead, and has none while its agents
 * have their own rows. `run` serializes actions and reports their errors.
 */
export function AccountDeviceRow({
  device,
  disabled,
  isNew = false,
  run,
  onChanged,
  onSignOut,
  publishRobot = publishDeviceRobot,
}: {
  device: AuthDevice;
  disabled: boolean;
  /** The device a "New sign-in" notice was about. */
  isNew?: boolean;
  /** `label` is the message shown if the action fails without its own error. */
  run: (label: MessageKey, action: () => Promise<unknown>) => Promise<void>;
  onChanged: () => Promise<void>;
  onSignOut: () => void;
  /** Injection seam for tests; publishes the kind:30181 robot choice. */
  publishRobot?: typeof publishDeviceRobot;
}) {
  const t = useT();
  const queryClient = useQueryClient();
  const ownerPubkey = useIdentityQuery().data?.pubkey;
  const { robotOverrides } = useOwnerDevices();
  const variant = resolveDeviceRobotVariant(
    device.id,
    robotOverrides.get(normalizeDeviceId(device.id)),
  );
  const [draft, setDraft] = React.useState<string | null>(null);
  const trimmed = draft?.trim() ?? "";
  const external = device.platform === EXTERNAL_PLATFORM;
  const label = device.name;
  const noAction =
    device.current ||
    draft !== null ||
    (external && (device.agents?.length ?? 0) > 0);

  const save = () => {
    if (draft === null) return;
    if (trimmed.length === 0 || trimmed === device.name) {
      setDraft(null);
      return;
    }
    void run("settings.device.renameFailed", async () => {
      await renameAuthDevice(device.id, trimmed);
      setDraft(null);
      applyAuthDeviceRename(queryClient, device.id, trimmed);
      await onChanged();
    });
  };

  return (
    <li
      className={
        isNew
          ? "-mx-2 flex items-center justify-between gap-3 rounded-lg border border-border/80 bg-muted/40 px-2 py-1"
          : "flex items-center justify-between gap-3"
      }
      data-new-sign-in={isNew ? "true" : undefined}
    >
      <span className="flex min-w-0 flex-1 items-center gap-2 text-sm">
        <DeviceRobotPicker
          deviceId={device.id}
          deviceName={device.name}
          disabled={disabled || !ownerPubkey}
          onPick={(override) => {
            if (!ownerPubkey) return;
            void run("settings.device.changeRobotFailed", () =>
              publishRobot(queryClient, {
                ownerPubkey,
                deviceId: device.id,
                override,
              }),
            );
          }}
          variant={variant}
        />
        {draft === null ? (
          <>
            <span className="min-w-0 truncate">
              {label}
              {isNew ? (
                <span
                  className="ml-1.5 rounded-full bg-foreground px-1.5 py-0.5 align-middle text-2xs font-semibold text-background"
                  data-testid="device-new-badge"
                >
                  {t("settings.device.new")}
                </span>
              ) : null}
              <span className="text-muted-foreground/70">
                {" "}
                {t(
                  device.current
                    ? "settings.device.platformCurrent"
                    : "settings.device.platform",
                  { platform: device.platform },
                )}
              </span>
            </span>
            <Button
              aria-label={t("settings.device.rename", { name: device.name })}
              className="h-6 w-6 shrink-0"
              disabled={disabled}
              onClick={() => setDraft(device.name)}
              size="icon"
              type="button"
              variant="ghost"
            >
              <Pencil aria-hidden="true" className="h-3.5 w-3.5" />
            </Button>
          </>
        ) : (
          <form
            className="flex min-w-0 flex-1 items-center gap-2"
            onSubmit={(event) => {
              event.preventDefault();
              save();
            }}
          >
            <Input
              aria-label={t("settings.device.newName", { name: device.name })}
              autoFocus
              className="h-7 min-w-0 flex-1 text-sm"
              disabled={disabled}
              maxLength={DEVICE_NAME_MAX}
              onChange={(event) => setDraft(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Escape") {
                  event.preventDefault();
                  setDraft(null);
                }
              }}
              value={draft}
            />
            <Button
              disabled={disabled || trimmed.length === 0}
              size="sm"
              type="submit"
            >
              {t("common.save")}
            </Button>
            <Button
              disabled={disabled}
              onClick={() => setDraft(null)}
              size="sm"
              type="button"
              variant="ghost"
            >
              {t("common.cancel")}
            </Button>
          </form>
        )}
      </span>
      {noAction ? null : (
        <Button
          aria-label={t(
            external
              ? "settings.device.disconnectNamed"
              : "settings.device.signOutNamed",
            { name: label },
          )}
          data-testid={external ? `device-disconnect-${device.id}` : undefined}
          disabled={disabled}
          onClick={onSignOut}
          size="sm"
          type="button"
          variant="outline"
        >
          {t(
            external ? "settings.device.disconnect" : "settings.device.signOut",
          )}
        </Button>
      )}
    </li>
  );
}

/** One external agent, under the row of the computer it runs on. */
export function ExternalAgentRow({
  agent,
  disabled,
  hermesPubkeys,
  onDisconnect,
}: {
  agent: AuthDeviceAgent;
  disabled: boolean;
  hermesPubkeys: ReadonlySet<string>;
  /** `name`: the agent's name. */
  onDisconnect: (name: string) => void;
}) {
  const t = useT();
  const name = useExternalAgentName(agent.pubkey, hermesPubkeys);
  const runtime = agentRuntime(agent, hermesPubkeys);
  const label = runtime
    ? t("settings.device.externalAgentRuntime", { runtime })
    : t("settings.device.externalAgent");
  const named = name !== runtime && name !== t("agents.external.agentFallback");
  return (
    <li className="flex items-center justify-between gap-3 pl-8">
      <span className="min-w-0 truncate text-sm">
        {label}
        {named ? (
          <span className="text-muted-foreground/70"> {name}</span>
        ) : null}
      </span>
      <Button
        aria-label={t("settings.device.disconnectNamed", { name })}
        data-testid={`agent-disconnect-${agent.pubkey}`}
        disabled={disabled}
        onClick={() => onDisconnect(name)}
        size="sm"
        type="button"
        variant="outline"
      >
        {t("settings.device.disconnect")}
      </Button>
    </li>
  );
}
