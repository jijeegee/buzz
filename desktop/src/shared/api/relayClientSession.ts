import { t } from "@/shared/i18n";
import { Channel, invoke } from "@tauri-apps/api/core";
import { buildRelayAuthFrame } from "@/shared/api/relayAuthFrame";
import { relayConnectAllowed } from "@/shared/api/relayConnectGate";
import { getRelayWsUrl, signRelayEvent } from "@/shared/api/tauri";
import type { PresenceStatus, RelayEvent } from "@/shared/api/types";
import {
  KIND_STREAM_MESSAGE,
  KIND_TYPING_INDICATOR,
  KIND_USER_STATUS,
} from "@/shared/constants/kinds";
import {
  getTextPayload,
  liveReqFrame,
  parseRelayFrame,
  toRelayFrames,
  type ConnectionState,
  type LiveSubscriptionReadiness,
  type PendingEvent,
  type RelaySubscription,
  type RelaySubscriptionFilter,
  type SubscriptionEventBufferItem,
} from "@/shared/api/relayClientShared";
import {
  buildChannelAuxDeletionFilter,
  buildChannelHistoryFilter,
  buildGlobalStreamFilter,
} from "@/shared/api/relayChannelFilters";
import {
  retireLiveSubscription,
  flushEvents,
  handleRelayClosed,
  handleSubscriptionEose,
  prepareSubscriptionEvent,
} from "@/shared/api/relayClosedRecovery";
import { getChannelReconnectRepairEvents } from "@/shared/api/channelReconnectRepair";
import { replayUntilRequestsRestored } from "@/shared/api/relayReconnectReplay";
import {
  type LiveSessionHost,
  sendRawWithReconnectRetry,
  subscribeLiveSession,
} from "@/shared/api/relayLiveSubscribe";
import {
  subscribeLiveFilters,
  subscribeRoomLiveFilters,
} from "@/shared/api/relayLiveFilters";
import { RoomLiveBundle } from "@/shared/api/relayRoomBundle";
import {
  UserStreamSession,
  userStreamEnabled,
} from "@/shared/api/relayUserStream";
import {
  attachUserStreamLive,
  tauriStreamStore,
} from "@/shared/api/relayUserStreamStore";
import { RelayLiveReqDrain } from "./relayLiveReqDrain";
import { RelayChannelAccessRevocations } from "./relayChannelAccessRevocations";
import { publishSessionEvent } from "@/shared/api/relayEventPublisher";
import { activateRateLimitIfSignalled } from "@/shared/api/relayRateLimitGate";
import {
  fetchChunkedHistory,
  requestFirstEventGated,
  requestHistoryGated,
} from "@/shared/api/relayGateBoundary";
import { RelayConnectionStateEmitter } from "@/shared/api/relayConnectionStateEmitter";
import {
  isServiceRestartClose,
  isWebSocketClose,
  isWebSocketError,
  shouldRefuseConnect,
  shouldScheduleReconnect,
  shouldWaitForScheduledReconnect,
} from "@/shared/api/relayReconnectPolicy";
import { RelayReconnectWaiters } from "@/shared/api/relayReconnectWaiters";
import { RelayStallWatchdog } from "@/shared/api/relayStallWatchdog";
import {
  AUTH_TIMEOUT_MS,
  BACKOFF_RESET_STABLE_MS,
  EVENT_BATCH_MS,
  HISTORY_TIMEOUT_MS,
  RECONNECT_BASE_DELAY_MS,
  RECONNECT_MAX_DELAY_MS,
  STALL_CHECK_INTERVAL_MS,
  STALL_IDLE_TIMEOUT_MS,
} from "@/shared/api/relayClientTimings";
import { closeWebSocket } from "@/shared/api/relayWebSocketClose";
import { noteSignedOutRefusal } from "@/shared/api/remoteSignOut";
import {
  armRelayAuthentication,
  AuthOkTracker,
  type RelayAuthRequest,
} from "@/shared/api/relayAuthPolicy";
import { createRelayInboundBuffer } from "@/shared/api/relayInboundBuffer";
import { buildThreadReferenceTags } from "@/features/messages/lib/threading";
type UserStatusInput = { text: string; emoji: string; expiresAt?: number };
/** A send time the sender saw, kept only if it is within the last two minutes. */
function recentSendTime(createdAt: number | undefined): number | undefined {
  if (createdAt === undefined) return undefined;
  const now = Math.floor(Date.now() / 1_000);
  return createdAt <= now && now - createdAt <= 120 ? createdAt : undefined;
}

const INTERACTIVE = [
  undefined,
  undefined,
  undefined,
  undefined,
  "interactive",
] as const;
export class RelayClient {
  private wsId: number | null = null;
  private relayUrl: string | null = null;
  private connectPromise: Promise<number> | null = null;
  private reconnectTimeout: number | null = null;
  private reconnectWaiters = new RelayReconnectWaiters();
  private reconnectDelayMs = RECONNECT_BASE_DELAY_MS;
  private keepAliveRequested = false;
  private authRequest: RelayAuthRequest | null = null;
  private subscriptions = new Map<string, RelaySubscription>();
  private liveReqDrain = new RelayLiveReqDrain();
  private pendingEvents = new Map<string, PendingEvent>();
  private eventBuffer: SubscriptionEventBufferItem[] = [];
  private flushTimeout: number | null = null;
  private reconnectListeners = new Set<() => void>();
  private channelAccessRevocations = new RelayChannelAccessRevocations();
  private hasConnectedOnce = false;
  private notifyReconnectListeners = false;
  private onMessageChannel: Channel<unknown> | null = null;
  private connectionGeneration = 0;
  private sessionEpoch = 0;
  private liveSessionAbort = new AbortController();
  private stabilityTimer: number | null = null;
  private visibleChannelId: string | null = null;
  /** Every room's live events ride this bundle (one REQ per 128 rooms). */
  readonly liveRooms = new RoomLiveBundle(
    (filter, onEvent, onReady, signal) =>
      this.subscribeLive(filter, onEvent, onReady, undefined, signal),
    {
      setTimeout: (fn, ms) => window.setTimeout(fn, ms),
      clearTimeout: (id) => window.clearTimeout(id),
    },
  );
  /** The account's user stream, sent first after AUTH OK (`relayUserStream.ts`). */
  readonly userStream = new UserStreamSession({
    generation: () => this.connectionGeneration,
    send: (payload, generation) =>
      this.sendRawForGeneration(payload, generation),
    closeSocket: (generation) => {
      if (generation === this.connectionGeneration)
        this.resetConnection(new Error("Relay ended the user stream."));
    },
    chunkRooms: () => this.liveRooms.chunkRooms(),
    roomOf: (event) => this.liveRooms.streamRoomOf(event),
    nowMs: () => Date.now(),
    setTimeout: (fn, ms) => window.setTimeout(fn, ms),
    clearTimeout: (id) => window.clearTimeout(id),
    log: (line) => console.info(line),
    enabled: userStreamEnabled,
    loadState: tauriStreamStore.getState,
    saveState: (key, value) =>
      void tauriStreamStore.setState(key, value).catch(() => {}),
  });
  readonly userStreamLive = attachUserStreamLive(
    this.userStream,
    this.liveRooms,
    () => this.channelAccessRevocations.hint(),
    () => this.visibleChannelId,
  );
  readonly stopShadowCompare = this.liveRooms.observeChunkEvents(
    (event, roomId) => this.userStream.chunkEvent(event, roomId),
  );
  private authOkTracker = new AuthOkTracker();
  private terminal = false;
  /** The session as live REQs see it (`relayLiveSubscribe.ts`). */
  private readonly liveHost: LiveSessionHost = {
    subscriptions: this.subscriptions,
    liveReqDrain: this.liveReqDrain,
    sessionEpoch: () => this.sessionEpoch,
    sessionSignal: () => this.liveSessionAbort.signal,
    connectionGeneration: () => this.connectionGeneration,
    visibleChannelId: () => this.visibleChannelId,
    ensureConnected: () => this.ensureConnected(),
    sendRawForGeneration: (payload, generation) =>
      this.sendRawForGeneration(payload, generation),
    recoverFromSocketFailure: (error, fallback) =>
      this.recoverFromSocketFailure(error, fallback),
    closeSubscription: (subId) => this.closeSubscription(subId),
  };

  private connectionStateEmitter = new RelayConnectionStateEmitter("idle");
  private stallWatchdog = new RelayStallWatchdog({
    intervalMs: STALL_CHECK_INTERVAL_MS,
    idleTimeoutMs: STALL_IDLE_TIMEOUT_MS,
    onStall: (error) => {
      this.connectionStateEmitter.set("stalled");
      this.resetConnection(error);
    },
  });
  setVisibleChannelId(id: string | null) {
    this.visibleChannelId = id;
  }
  disconnect() {
    const error = new Error("Relay disconnected for community switch.");

    if (this.reconnectTimeout) {
      window.clearTimeout(this.reconnectTimeout);
      this.reconnectTimeout = null;
    }
    if (this.stabilityTimer !== null) {
      window.clearTimeout(this.stabilityTimer);
      this.stabilityTimer = null;
    }
    this.stallWatchdog.stop();
    this.sessionEpoch++;
    this.liveSessionAbort.abort();
    this.liveSessionAbort = new AbortController();
    this.liveRooms.reset();
    this.userStream.socketEnded();
    this.userStream.scopeChanged();
    this.connectionGeneration++;
    this.liveReqDrain.reset();
    this.keepAliveRequested = false;
    this.relayUrl = null;
    this.hasConnectedOnce = false;
    this.notifyReconnectListeners = false;
    this.terminal = false;
    this.visibleChannelId = null;
    this.authOkTracker.reset();
    this.connectionStateEmitter.set("idle");

    if (this.wsId !== null) {
      void closeWebSocket(this.wsId, "community switch");
      this.wsId = null;
    }

    this.connectPromise = null;
    this.reconnectWaiters.settle(error);

    if (this.authRequest) {
      window.clearTimeout(this.authRequest.timeout);
      this.authRequest.reject(error);
      this.authRequest = null;
    }

    for (const [subId, sub] of this.subscriptions) {
      if (sub.mode !== "live") {
        window.clearTimeout(sub.timeout);
        sub.reject(error);
      } else {
        retireLiveSubscription(sub);
        sub.onRemoved?.();
      }
      this.subscriptions.delete(subId);
    }

    for (const [eventId, pending] of this.pendingEvents) {
      window.clearTimeout(pending.timeout);
      pending.reject(error);
      this.pendingEvents.delete(eventId);
    }

    if (this.flushTimeout !== null) {
      window.clearTimeout(this.flushTimeout);
      this.flushTimeout = null;
    }
    this.eventBuffer = [];
    this.reconnectListeners.clear();
    this.channelAccessRevocations.clear();
    this.connectionStateEmitter.clear();
    this.onMessageChannel = null;
    this.reconnectDelayMs = RECONNECT_BASE_DELAY_MS;
  }

  async fetchChannelHistory(channelId: string, limit = 50) {
    return this.fetchHistory(buildChannelHistoryFilter(channelId, limit));
  }

  async fetchChannelHistoryBefore(
    channelId: string,
    before: number,
    limit = 50,
  ) {
    return this.fetchHistory(
      buildChannelHistoryFilter(channelId, limit, before),
    );
  }

  async fetchAuxEventsByReference(
    channelId: string,
    referencedEventIds: string[],
    buildFilter: (
      channelId: string,
      eventIds: string[],
    ) => RelaySubscriptionFilter,
  ) {
    return fetchChunkedHistory(
      referencedEventIds,
      (eventIds) => buildFilter(channelId, eventIds),
      (filter) => this.fetchHistory(filter),
    );
  }

  async fetchAuxDeletionEventsForAuxEvents(
    channelId: string,
    auxEventIds: string[],
  ): Promise<RelayEvent[]> {
    return fetchChunkedHistory(
      auxEventIds,
      (eventIds) => buildChannelAuxDeletionFilter(channelId, eventIds),
      (filter) => this.fetchHistory(filter),
    );
  }

  async fetchEvents(filter: RelaySubscriptionFilter): Promise<RelayEvent[]> {
    return this.fetchHistory(filter);
  }

  async fetchFirstEvent(
    filter: RelaySubscriptionFilter,
  ): Promise<RelayEvent | null> {
    await this.ensureConnected();
    return requestFirstEventGated(
      this.subscriptions,
      (payload) => this.sendRaw(payload),
      (subId) => this.closeSubscription(subId),
      filter,
      HISTORY_TIMEOUT_MS,
    );
  }

  private async fetchHistory(filter: RelaySubscriptionFilter) {
    await this.ensureConnected();
    return this.requestHistory(filter);
  }

  private requestHistory(
    filter: RelaySubscriptionFilter,
  ): Promise<RelayEvent[]> {
    return requestHistoryGated(
      this.subscriptions,
      (payload) => this.sendRaw(payload),
      (subId) => this.closeSubscription(subId),
      filter,
      HISTORY_TIMEOUT_MS,
    );
  }

  async sendMessage(
    channelId: string,
    content: string,
    mentionPubkeys: string[] = [],
    extraTags: string[][] = [],
    createdAt?: number,
  ) {
    await this.ensureConnected();

    const tags: string[][] = [["h", channelId]];
    for (const pubkey of mentionPubkeys) {
      tags.push(["p", pubkey]);
    }
    for (const tag of extraTags) {
      tags.push(tag);
    }

    const event = await signRelayEvent({
      kind: KIND_STREAM_MESSAGE,
      content: content.trim(),
      createdAt: recentSendTime(createdAt),
      tags,
    });

    return this.publishEvent(
      event,
      t("ui.message.sendTimedOut"),
      t("ui.message.sendFailed"),
    );
  }

  async sendPresence(status: PresenceStatus) {
    await this.ensureConnected();

    const event = await signRelayEvent({
      kind: 20001,
      content: status,
      tags: [],
    });

    return this.publishEvent(
      event,
      t("ui.presence.updateTimedOut"),
      t("ui.presence.updateFailed"),
    );
  }

  async sendTypingIndicator(
    channelId: string,
    parentEventId?: string | null,
    rootEventId?: string | null,
  ) {
    // Disconnected: not worth triggering a reconnect for ephemeral typing.
    if (this.wsId === null) {
      return;
    }
    const event = await signRelayEvent({
      kind: KIND_TYPING_INDICATOR,
      content: "",
      tags: buildThreadReferenceTags(
        channelId,
        parentEventId ?? null,
        rootEventId ?? null,
      ),
    });

    // Fire-and-forget: no need to wait for relay acknowledgement.
    void this.sendRaw(["EVENT", event]).catch(() => {});
  }

  /** One room's live rows, aux and 39005 summaries, riding `liveRooms`. */
  async subscribeToChannelLive(
    channelId: string,
    onEvent: (event: RelayEvent) => void,
  ) {
    return this.liveRooms.listenRoom(channelId, onEvent);
  }

  async publishUserStatus(status: UserStatusInput): Promise<RelayEvent> {
    await this.ensureConnected();
    const tags: string[][] = [["d", "general"]];
    if (status.emoji) tags.push(["emoji", status.emoji]);
    if (status.expiresAt) tags.push(["expiration", String(status.expiresAt)]);
    const event = await signRelayEvent({
      kind: KIND_USER_STATUS,
      content: status.text,
      tags,
    });
    return this.publishEvent(
      event,
      t("ui.userStatus.publishTimedOut"),
      t("ui.userStatus.publishFailed"),
    );
  }

  async subscribeToAllStreamMessages(onEvent: (event: RelayEvent) => void) {
    return this.subscribe(buildGlobalStreamFilter(50), onEvent);
  }

  async subscribeLive(
    filter: RelaySubscriptionFilter,
    onEvent: (event: RelayEvent) => void,
    onReady?: (readiness: LiveSubscriptionReadiness) => void,
    timeoutMs?: number,
    signal?: AbortSignal,
    gone?: () => void, // dropped for good: disposed or a terminal CLOSED
  ) {
    return this.subscribe(filter, onEvent, onReady, timeoutMs, signal, gone);
  }
  /** One live REQ with several `limit: 0` filters, for a bundle. */
  async subscribeLiveFilters(
    filters: RelaySubscriptionFilter[],
    onEvent: (event: RelayEvent) => void,
    onReady?: (readiness: LiveSubscriptionReadiness) => void,
    signal?: AbortSignal,
    gone?: () => void,
  ) {
    return subscribeLiveFilters(
      this.liveHost,
      (payload, fallback, onDispatch) =>
        this.sendRawWithReconnectRetry(payload, fallback, onDispatch),
      filters,
      onEvent,
      onReady,
      signal,
      gone,
    );
  }
  /** One live REQ with several `limit: 0` filters for one room. */
  async subscribeRoomLiveFilters(
    filters: RelaySubscriptionFilter[],
    onEvent: (event: RelayEvent) => void,
    onReady?: (readiness: LiveSubscriptionReadiness) => void,
    signal?: AbortSignal,
    gone?: () => void,
  ) {
    return subscribeRoomLiveFilters(
      this.liveHost,
      (payload, fallback, onDispatch) =>
        this.sendRawWithReconnectRetry(payload, fallback, onDispatch),
      filters,
      onEvent,
      onReady,
      signal,
      gone,
    );
  }
  /** Prioritize an interactive live consumer without changing its replay filter or pacing. */
  async subscribeInteractive(
    filter: RelaySubscriptionFilter,
    onEvent: (event: RelayEvent) => void,
  ) {
    return this.subscribe(filter, onEvent, ...INTERACTIVE);
  }

  async preconnect() {
    // Explicit re-engagement (reconnect card / community switch): clears the
    // terminal latch and AUTH rejection streak, and bypasses backoff once.
    this.terminal = false;
    this.authOkTracker.reset();
    this.keepAliveRequested = true;
    await this.connectBypassingBackoff();
  }

  /**
   * Environment-driven resume (online/focus/visibility): bypasses a pending
   * backoff timer but preserves the terminal latch and AUTH rejection streak
   * — only `preconnect()` clears those, so resume events during repeated
   * AUTH rejection cannot defeat the consecutive-rejection cap.
   */
  async resumeReconnect() {
    if (this.terminal) return;
    await this.connectBypassingBackoff();
  }

  private async connectBypassingBackoff() {
    if (this.reconnectTimeout !== null) {
      window.clearTimeout(this.reconnectTimeout);
      this.reconnectTimeout = null;
    }
    try {
      await this.ensureConnected();
      this.reconnectWaiters.settle();
    } catch (error) {
      this.reconnectWaiters.settle(
        this.normalizeRelayError(error, "Relay reconnect failed."),
      );
      throw error;
    }
  }

  subscribeToReconnects(listener: () => void) {
    this.reconnectListeners.add(listener);
    return () => {
      this.reconnectListeners.delete(listener);
    };
  }

  /** Listen for channel access-revocation hints; authoritative state needs a refresh. */
  subscribeToChannelAccessRevocations(listener: () => void) {
    return this.channelAccessRevocations.subscribe(listener);
  }

  /** Current connection state — synchronous read. */
  getConnectionState(): ConnectionState {
    return this.connectionStateEmitter.get();
  }

  /**
   * Subscribe to connection-state transitions. The listener fires
   * immediately with the current state, so callers need no separate
   * `getConnectionState()` call to seed their UI.
   */
  subscribeToConnectionState(listener: (state: ConnectionState) => void) {
    return this.connectionStateEmitter.subscribe(listener);
  }

  private async ensureConnected() {
    if (shouldRefuseConnect({ terminal: this.terminal })) {
      // Terminal (e.g. relay rejected auth): refuse until disconnect() or
      // preconnect() clears the latch, else the reconnect-timer catch and
      // the publish/subscribe retry wrappers would race the terminal
      // "disconnected" state back to "reconnecting".
      throw new Error("Relay session is terminal; cannot reconnect.");
    }

    if (this.connectPromise) {
      return this.connectPromise;
    }

    if (this.wsId !== null) {
      return this.connectionGeneration;
    }

    if (
      shouldWaitForScheduledReconnect({
        hasPendingReconnect: this.reconnectTimeout !== null,
      })
    ) {
      // The reconnect coordinator owns outage pacing. Query, publish, and
      // subscription callers must wait for its scheduled attempt instead of
      // clearing the timer and creating an immediate reconnect storm.
      return this.reconnectWaiters.wait().then(() => this.connectionGeneration);
    }

    const connectPromise = this.connect();
    this.connectPromise = connectPromise;

    try {
      return await connectPromise;
    } finally {
      if (this.connectPromise === connectPromise) {
        this.connectPromise = null;
      }
    }
  }

  private async connect() {
    if (this.stabilityTimer !== null) {
      window.clearTimeout(this.stabilityTimer);
      this.stabilityTimer = null;
    }

    this.connectionStateEmitter.set(
      this.hasConnectedOnce ? "reconnecting" : "connecting",
    );
    const generation = ++this.connectionGeneration;
    const inbound = createRelayInboundBuffer(
      async (delivery) => {
        for (const message of toRelayFrames(delivery))
          await this.handleWsMessage(message, generation);
      },
      (error) => {
        if (generation === this.connectionGeneration)
          this.recoverFromSocketFailure(error, "Relay connection errored.");
      },
    );
    this.onMessageChannel = new Channel<unknown>((delivery) =>
      inbound.receive(delivery),
    );
    try {
      await relayConnectAllowed();
      this.relayUrl ||= await getRelayWsUrl();
      const wsId = await invoke<number>("plugin:websocket|connect", {
        url: this.relayUrl,
        onMessage: this.onMessageChannel,
        config: {},
      });
      if (generation !== this.connectionGeneration) {
        void closeWebSocket(wsId, "stale connection attempt");
        throw new Error("Relay connection attempt was superseded.");
      }
      this.wsId = wsId;

      const authentication = armRelayAuthentication(
        AUTH_TIMEOUT_MS,
        (request) => {
          this.authRequest = request;
        },
        (error) => {
          this.authRequest = null;
          this.resetConnection(error);
        },
      );

      const drain = inbound.drain();
      await Promise.race([drain, authentication, inbound.overflow]);
      await drain;
      await authentication;
      this.stabilityTimer = window.setTimeout(() => {
        this.stabilityTimer = null;
        this.reconnectDelayMs = RECONNECT_BASE_DELAY_MS;
      }, BACKOFF_RESET_STABLE_MS);

      this.connectionStateEmitter.set("connected");
      await this.userStream.connected(generation);
      await this.replayLiveSubscriptions();
      this.stallWatchdog.start();
      this.emitReconnectIfNeeded();
      return generation;
    } catch (error) {
      const connectionError = this.normalizeRelayError(
        error,
        "Failed to connect to relay.",
      );
      if (generation === this.connectionGeneration) {
        this.resetConnection(connectionError);
      }
      throw connectionError;
    }
  }

  private subscribe(
    filter: RelaySubscriptionFilter,
    onEvent: (event: RelayEvent) => void,
    onReady?: (readiness: LiveSubscriptionReadiness) => void,
    readinessTimeoutMs = 250,
    signal?: AbortSignal,
    onGone?: () => void,
    priority?: "interactive",
  ) {
    return subscribeLiveSession(
      this.liveHost,
      (payload, fallback, onDispatch) =>
        this.sendRawWithReconnectRetry(payload, fallback, onDispatch),
      filter,
      onEvent,
      onReady,
      readinessTimeoutMs,
      signal,
      onGone,
      priority,
    );
  }

  private async sendRaw(payload: unknown[]) {
    if (this.wsId === null) {
      throw new Error("Relay socket is not connected.");
    }

    await invoke("plugin:websocket|send", {
      id: this.wsId,
      message: {
        type: "Text",
        data: JSON.stringify(payload),
      },
    });
  }

  private async sendRawForGeneration(payload: unknown[], generation: number) {
    if (generation !== this.connectionGeneration || this.wsId === null) {
      throw new Error("Relay publish was superseded by a session change.");
    }
    const wsId = this.wsId;
    await invoke("plugin:websocket|send", {
      id: wsId,
      message: { type: "Text", data: JSON.stringify(payload) },
    });
  }

  private normalizeRelayError(error: unknown, fallbackMessage: string) {
    return error instanceof Error ? error : new Error(fallbackMessage);
  }

  private recoverFromSocketFailure(
    error: unknown,
    fallbackMessage: string,
  ): Error {
    const normalizedError = this.normalizeRelayError(error, fallbackMessage);
    this.resetConnection(normalizedError);
    return normalizedError;
  }

  private sendRawWithReconnectRetry(
    payload: unknown[],
    fallbackMessage: string,
    onDispatch?: () => void,
  ) {
    return sendRawWithReconnectRetry(
      this.liveHost,
      payload,
      fallbackMessage,
      onDispatch,
    );
  }

  private async closeSubscription(subId: string) {
    if (this.wsId === null) {
      return;
    }

    await this.sendRaw(["CLOSE", subId]);
  }

  async publishEvent(
    event: RelayEvent,
    timeoutMessage: string,
    sendErrorMessage: string,
    isCurrent?: () => boolean,
  ) {
    return publishSessionEvent(
      {
        generation: () => this.connectionGeneration,
        ownership: () => this.sessionEpoch,
        pendingEvents: this.pendingEvents,
        send: (payload, generation) =>
          this.sendRawForGeneration(payload, generation),
        reconnect: () => this.ensureConnected(),
        normalizeError: (error, fallback) =>
          this.normalizeRelayError(error, fallback),
        recoverSocketFailure: (error, fallback) =>
          this.recoverFromSocketFailure(error, fallback),
      },
      event,
      timeoutMessage,
      sendErrorMessage,
      isCurrent,
    );
  }

  private async handleWsMessage(message: unknown, generation: number) {
    if (generation !== this.connectionGeneration) return;
    this.stallWatchdog.recordInbound();

    if (isWebSocketClose(message)) {
      if (isServiceRestartClose(message))
        this.reconnectDelayMs = RECONNECT_BASE_DELAY_MS;
      this.resetConnection(new Error("Relay connection closed."));
      return;
    }
    if (isWebSocketError(message)) {
      this.resetConnection(new Error("Relay connection errored."));
      return;
    }

    const payload = getTextPayload(message);
    if (!payload) {
      return;
    }

    const data = parseRelayFrame(payload);
    if (!data) return;
    const [type, ...rest] = data;
    if (type === "AUTH" && typeof rest[0] === "string") {
      await this.handleAuthChallenge(rest[0], generation);
      return;
    }
    if (typeof rest[0] === "string" && this.userStream.handles(rest[0])) {
      if (type === "EVENT" && rest[1])
        this.userStream.event(rest[0], rest[1] as RelayEvent);
      else if (type === "EOSE") this.userStream.eose(rest[0]);
      else if (type === "CLOSED")
        this.userStream.closed(rest[0], String(rest[1] ?? ""));
      return;
    }
    if (type === "EVENT" && typeof rest[0] === "string" && rest[1]) {
      this.handleEvent(rest[0], rest[1] as RelayEvent, generation);
      return;
    }

    if (
      type === "OK" &&
      typeof rest[0] === "string" &&
      typeof rest[1] === "boolean"
    ) {
      this.handleOk(
        rest[0],
        rest[1],
        typeof rest[2] === "string" ? rest[2] : "",
      );
      return;
    }

    if (type === "EOSE" && typeof rest[0] === "string") {
      this.handleEose(rest[0], generation);
      return;
    }
    if (type === "CLOSED" && typeof rest[0] === "string") {
      const subscription = this.subscriptions.get(rest[0]);
      handleRelayClosed({
        subscriptions: this.subscriptions,
        subId: rest[0],
        message: typeof rest[1] === "string" ? rest[1] : "",
        sendReq: (subId, filter) =>
          this.sendRawWithReconnectRetry(
            liveReqFrame(subId, this.subscriptions.get(subId), filter),
            "Failed to restore relay subscription after CLOSED.",
          ),
        closeSubscription: (subId) => this.closeSubscription(subId),
      });
      if (!this.subscriptions.has(rest[0])) this.liveReqDrain.cancel(rest[0]);
      this.channelAccessRevocations.notify(subscription, rest[1]);
      return;
    }

    if (type === "NOTICE" && typeof rest[0] === "string") {
      // Connection-scoped back-pressure — arm the gate until it expires.
      activateRateLimitIfSignalled(rest[0]);
      noteSignedOutRefusal(rest[0]);
    }
  }

  private async handleAuthChallenge(challenge: string, generation: number) {
    if (!this.relayUrl) {
      this.relayUrl = await getRelayWsUrl();
    }

    // Token AUTH in a Google-session community, NIP-42 otherwise.
    const frame = await buildRelayAuthFrame({
      challenge,
      relayUrl: this.relayUrl,
    });

    if (generation !== this.connectionGeneration || !this.authRequest) {
      return;
    }

    this.authRequest.pendingEventId = frame.pendingId;
    await this.sendRaw(["AUTH", frame.payload]);
  }

  private handleEvent(subId: string, event: RelayEvent, generation: number) {
    const subscription = this.subscriptions.get(subId);
    if (!subscription) {
      return;
    }

    if (subscription.mode === "first") {
      subscription.onEvent(event);
      return;
    }

    if (!prepareSubscriptionEvent(subscription, event)) return;
    if (subscription.mode === "live") this.userStreamLive.noteLive(event.id);
    if (event.kind === 44100 || event.kind === 44101) {
      if (subscription.mode === "live") this.userStream.membershipNotice(event);
    }
    this.liveReqDrain.cancel(subId);
    this.eventBuffer.push({ subId, event, generation });
    this.flushTimeout ??= window.setTimeout(
      () => this.flushEventBuffer(),
      EVENT_BATCH_MS,
    );
  }

  private flushEventBuffer() {
    this.flushTimeout = null;
    const buffer = this.eventBuffer;
    this.eventBuffer = [];

    flushEvents(buffer, this.subscriptions, this.connectionGeneration);
  }

  private handleEose(subId: string, generation: number) {
    this.liveReqDrain.cancel(subId);
    this.flushEventBuffer(); // Deliver preceding EVENT frames before EOSE.
    handleSubscriptionEose({
      subscriptions: this.subscriptions,
      subId,
      closeSubscription: (id) => this.closeSubscription(id),
      generation,
    });
  }

  private handleOk(eventId: string, success: boolean, message: string) {
    if (this.authRequest && this.authRequest.pendingEventId === eventId) {
      window.clearTimeout(this.authRequest.timeout);
      const authRequest = this.authRequest;
      this.authRequest = null;

      if (!success) noteSignedOutRefusal(message); // Confirmed with the server.
      // Decision table lives in relayAuthPolicy.ts.
      const decision = this.authOkTracker.record(success, message);
      if (decision === "authenticated") {
        authRequest.resolve();
        this.userStream.authOk(this.connectionGeneration);
      } else {
        const error = new Error(message || "Relay authentication rejected.");
        authRequest.reject(error);
        this.resetConnection(error, { reconnect: decision === "retry" });
      }

      return;
    }

    const pendingEvent = this.pendingEvents.get(eventId);
    if (!pendingEvent) {
      return;
    }

    window.clearTimeout(pendingEvent.timeout);
    this.pendingEvents.delete(eventId);

    if (success) {
      pendingEvent.resolve(pendingEvent.event);
    } else {
      // Back-pressure now arrives here rather than as a NOTICE: the relay
      // rejects an over-quota EVENT on the OK channel so this pending publish
      // can be settled at all. Unarmed, the send retries into the same quota.
      activateRateLimitIfSignalled(message);
      pendingEvent.reject(new Error(message || "Relay rejected the event."));
    }
  }

  private hasLiveSubscriptions() {
    return [...this.subscriptions.values()].some((s) => s.mode === "live");
  }

  /** Resolves once live REQs are re-sent; repair walks continue behind. */
  private replayLiveSubscriptions() {
    const generation = this.connectionGeneration;
    return replayUntilRequestsRestored(
      {
        subscriptions: this.subscriptions,
        sendRaw: (payload) => this.sendRaw(payload),
        requestRepair: getChannelReconnectRepairEvents,
        generation,
        visibleChannelId: this.visibleChannelId,
        isActive: () => this.connectionGeneration === generation,
      },
      (error) => {
        // A late failure must not tear down a newer connection.
        if (this.connectionGeneration === generation)
          this.resetConnection(error);
      },
    );
  }

  private scheduleReconnect() {
    if (
      !shouldScheduleReconnect({
        terminal: this.terminal,
        hasPendingReconnect: this.reconnectTimeout !== null,
        hasLiveSocket: this.wsId !== null,
        keepAliveRequested: this.keepAliveRequested,
        hasLiveSubscriptions: this.hasLiveSubscriptions(),
      })
    ) {
      return;
    }

    // ±25% jitter spreads a fleet's AUTH storms across a 50% window instead
    // of hitting the relay at the same instant.
    const jitter = this.reconnectDelayMs * (0.75 + Math.random() * 0.5);
    const delay =
      Math.min(jitter, RECONNECT_MAX_DELAY_MS) +
      this.userStream.takeReconnectJitterMs();
    this.reconnectDelayMs = Math.min(
      this.reconnectDelayMs * 2,
      RECONNECT_MAX_DELAY_MS,
    );

    this.reconnectTimeout = window.setTimeout(() => {
      this.reconnectTimeout = null;
      void this.ensureConnected()
        .then(() => this.reconnectWaiters.settle())
        .catch((error) => {
          this.reconnectWaiters.settle(
            this.normalizeRelayError(error, "Relay reconnect failed."),
          );
          this.scheduleReconnect();
        });
    }, delay);
  }

  private emitReconnectIfNeeded() {
    const shouldNotifyReconnectListeners =
      this.hasConnectedOnce && this.notifyReconnectListeners;

    this.hasConnectedOnce = true;
    this.notifyReconnectListeners = false;

    if (!shouldNotifyReconnectListeners) {
      return;
    }

    for (const listener of this.reconnectListeners) {
      try {
        listener();
      } catch (error) {
        console.error("Failed to handle relay reconnect", error);
      }
    }
  }

  private resetConnection(
    error: Error,
    options?: {
      reconnect?: boolean;
    },
  ) {
    this.onMessageChannel = null;
    this.stallWatchdog.stop();
    this.userStream.socketEnded();
    this.connectionGeneration++;
    this.liveReqDrain.reset();
    if (this.stabilityTimer !== null) {
      window.clearTimeout(this.stabilityTimer);
      this.stabilityTimer = null;
    }
    if (this.flushTimeout !== null) window.clearTimeout(this.flushTimeout);
    this.flushTimeout = null;
    this.eventBuffer = [];

    if (options?.reconnect === false) {
      this.terminal = true;
      this.connectionStateEmitter.set("disconnected");
    } else if (
      // A late retry failure racing a terminal latch must not paint
      // "reconnecting" over the terminal "disconnected" state; stall is a
      // stronger signal than a generic drop and is kept until reconnect.
      !this.terminal &&
      this.connectionStateEmitter.get() !== "stalled"
    ) {
      this.connectionStateEmitter.set("reconnecting");
    }

    if (options?.reconnect !== false && this.hasConnectedOnce) {
      this.notifyReconnectListeners = true;
    }

    if (options?.reconnect === false && this.reconnectTimeout) {
      window.clearTimeout(this.reconnectTimeout);
      this.reconnectTimeout = null;
    }
    if (options?.reconnect === false) {
      this.reconnectWaiters.settle(error);
    }

    if (this.wsId !== null) {
      void closeWebSocket(this.wsId, "connection reset");
    }

    this.wsId = null;

    if (this.authRequest) {
      window.clearTimeout(this.authRequest.timeout);
      this.authRequest.reject(error);
      this.authRequest = null;
    }

    for (const [subId, subscription] of this.subscriptions) {
      if (subscription.mode !== "live") {
        window.clearTimeout(subscription.timeout);
        subscription.reject(error);
        this.subscriptions.delete(subId);
        continue;
      }
      subscription.resolveReady?.("closed");
      subscription.resolveReady = undefined;
      retireLiveSubscription(subscription);
      subscription.multi?.lost();
    }
    for (const [eventId, pendingEvent] of this.pendingEvents) {
      window.clearTimeout(pendingEvent.timeout);
      pendingEvent.reject(error);
      this.pendingEvents.delete(eventId);
    }
    if (options?.reconnect !== false) {
      this.scheduleReconnect();
    }
  }
}
