import * as React from "react";
import {
  type QueryClient,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";

import { useIdentityQuery } from "@/shared/api/hooks";
import {
  buildOwnerDevices,
  EMPTY_OWNER_DEVICES,
  normalizeDeviceId,
  type OwnerDevices,
  type StoredDeviceRobotOverride,
} from "@/shared/api/ownerDevices";
import { relayClient } from "@/shared/api/relayClient";
import { signRelayEvent } from "@/shared/api/tauri";
import type { RelayEvent } from "@/shared/api/types";
import {
  KIND_AGENT_HOST_DEVICES,
  KIND_DEVICE_ROBOT,
} from "@/shared/constants/kinds";
import {
  type DeviceRobotOverride,
  deviceRobotContent,
} from "@/shared/lib/deviceRobot";
import { normalizePubkey } from "@/shared/lib/pubkey";

const OWNER_DEVICES_QUERY_KEY = ["owner-devices"] as const;
const OWNER_DEVICE_KINDS = [KIND_AGENT_HOST_DEVICES, KIND_DEVICE_ROBOT];

/** Upper bound on per-device records; two replaceable events per device. */
const OWNER_DEVICES_FETCH_LIMIT = 400;

/** The query client is per community, so the key only needs the author. */
export function ownerDevicesQueryKey(ownerPubkey: string) {
  return [...OWNER_DEVICES_QUERY_KEY, normalizePubkey(ownerPubkey)] as const;
}

export async function fetchOwnerDevices(
  ownerPubkey: string,
): Promise<OwnerDevices> {
  const owner = normalizePubkey(ownerPubkey);
  const events = await relayClient.fetchEvents({
    kinds: OWNER_DEVICE_KINDS,
    authors: [owner],
    limit: OWNER_DEVICES_FETCH_LIMIT,
  });
  return buildOwnerDevices(events, owner);
}

/**
 * The signed-in user's own device records (see `ownerDevices.ts`): which
 * device runs each of their agents, and the robot they picked per device.
 * Every caller shares one cached query; `enabled = false` (e.g. a viewer who
 * does not own the agent in question) reads the cache without fetching. Live
 * updates come from `useOwnerDevicesLiveUpdates`.
 */
export function useOwnerDevices(enabled = true): OwnerDevices {
  const identity = useIdentityQuery();
  const viewer = normalizePubkey(identity.data?.pubkey ?? "");
  const query = useQuery({
    enabled: enabled && viewer.length > 0,
    queryKey: ownerDevicesQueryKey(viewer),
    queryFn: () => fetchOwnerDevices(viewer),
    staleTime: 5 * 60_000,
  });
  return query.data ?? EMPTY_OWNER_DEVICES;
}

/**
 * Keep the owner's device records fresh: a live subscription to their own
 * kind:30180/30181 events (another of their computers starting an agent, or
 * a robot picked elsewhere) plus a refetch after reconnect, since events
 * published while disconnected do not replay. Mount once near the app root.
 */
export function useOwnerDevicesLiveUpdates(
  pubkey: string | null | undefined,
): void {
  const queryClient = useQueryClient();
  const owner = normalizePubkey(pubkey ?? "");

  React.useEffect(() => {
    if (!owner) return;
    const queryKey = ownerDevicesQueryKey(owner);
    const invalidate = () => {
      void queryClient.invalidateQueries({ queryKey });
    };
    let disposed = false;
    let dispose: (() => void) | undefined;

    void relayClient
      .subscribeLive(
        { kinds: OWNER_DEVICE_KINDS, authors: [owner], limit: 0 },
        invalidate,
      )
      .then((unsubscribe) => {
        if (disposed) {
          void unsubscribe();
        } else {
          dispose = () => {
            void unsubscribe();
          };
        }
      })
      .catch((error) => {
        console.error("Failed to subscribe to owner device records", error);
      });
    const unsubscribeReconnect = relayClient.subscribeToReconnects(invalidate);

    return () => {
      disposed = true;
      unsubscribeReconnect();
      dispose?.();
    };
  }, [owner, queryClient]);
}

function withRobotOverride(
  current: OwnerDevices | undefined,
  deviceId: string,
  override: StoredDeviceRobotOverride | null,
): OwnerDevices {
  const base = current ?? EMPTY_OWNER_DEVICES;
  const robotOverrides = new Map(base.robotOverrides);
  if (override) robotOverrides.set(deviceId, override);
  else robotOverrides.delete(deviceId);
  return { ...base, robotOverrides };
}

export type PublishDeviceRobotDeps = {
  sign: typeof signRelayEvent;
  publish: (event: RelayEvent) => Promise<unknown>;
  now: () => number;
};

const defaultPublishDeps: PublishDeviceRobotDeps = {
  sign: signRelayEvent,
  publish: (event) =>
    relayClient.publishEvent(
      event,
      "Timed out saving the device robot.",
      "Could not save the device robot.",
    ),
  now: () => Date.now(),
};

/**
 * Save the owner's robot choice for one of their devices as a kind:30181
 * event. The cached device records update first, so every robot for that
 * device (settings row, agent robots, tooltips) changes at once; a failed
 * publish restores the previous choice and rethrows.
 */
export async function publishDeviceRobot(
  queryClient: QueryClient,
  input: {
    ownerPubkey: string;
    deviceId: string;
    override: DeviceRobotOverride;
  },
  deps: PublishDeviceRobotDeps = defaultPublishDeps,
): Promise<void> {
  const deviceId = normalizeDeviceId(input.deviceId);
  if (!deviceId) throw new Error("Unknown device.");
  const queryKey = ownerDevicesQueryKey(input.ownerPubkey);
  await queryClient.cancelQueries({ queryKey });
  const previous = queryClient.getQueryData<OwnerDevices>(queryKey);
  const previousOverride = previous?.robotOverrides.get(deviceId) ?? null;
  // Replaceable events keep the newest; a re-pick within the same second
  // must still supersede the previous choice.
  const createdAt = Math.max(
    Math.floor(deps.now() / 1_000),
    (previousOverride?.createdAt ?? 0) + 1,
  );
  const optimistic = { ...input.override, createdAt };
  queryClient.setQueryData<OwnerDevices>(queryKey, (current) =>
    withRobotOverride(current, deviceId, optimistic),
  );
  try {
    const event = await deps.sign({
      kind: KIND_DEVICE_ROBOT,
      content: deviceRobotContent(input.override),
      createdAt,
      tags: [["d", deviceId]],
    });
    await deps.publish(event);
  } catch (error) {
    queryClient.setQueryData<OwnerDevices>(queryKey, (current) => {
      // Only roll back our own write; a newer pick or refetch stands.
      if (current?.robotOverrides.get(deviceId) !== optimistic) return current;
      return withRobotOverride(current, deviceId, previousOverride);
    });
    throw error;
  }
  // Reconcile with the persisted event (token mode re-signs it) on the next
  // read; the live subscription also refetches when the event comes back.
  // Refetch now only if the optimistic write created the cache entry, so the
  // host-device half of the records gets filled in.
  void queryClient.invalidateQueries({
    queryKey,
    refetchType: previous ? "none" : "active",
  });
}
