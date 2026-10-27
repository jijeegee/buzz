import { invoke } from "@tauri-apps/api/core";
import type { RelayEvent } from "./types";

export const MESSAGE_STORE_KINDS = new Set([
  5, 7, 9, 9005, 39005, 40001, 40002, 40003, 40008, 40099, 43001, 43002, 43003,
  43004, 43005, 43006, 45001, 45003, 48100, 48101, 48102, 48103,
]);

type Lease = { relay: string; account: string; epoch: number };
export type MessageCapture = {
  receive(event: RelayEvent): void;
  close(): void;
};

/** Diagnostics stay separate from socket health. Disk failure cannot stop live UI
 * delivery. The native store never marks these append-only scopes complete. */
export let messageStoreFailure: unknown = null;
function report(error: unknown) {
  messageStoreFailure = error;
  console.warn("Message storage incomplete; history repair required", error);
}

/** One capture per connection context; queued writes retain their original lease. */
export class MessageStoreCapture implements MessageCapture {
  private readonly lease: Promise<Lease | null>;
  private pending: RelayEvent[] = [];
  private bytes = 0;
  private draining: Promise<void> | null = null;
  private closed = false;

  private readonly call: typeof invoke;
  private readonly onError: (error: unknown) => void;
  constructor(
    call: typeof invoke = invoke,
    onError: (error: unknown) => void = report,
  ) {
    this.call = call;
    this.onError = onError;
    this.lease = call<Lease>("message_store_begin").catch((error) => {
      onError(error);
      return null;
    });
  }

  receive(event: RelayEvent) {
    if (this.closed || !MESSAGE_STORE_KINDS.has(event.kind)) return;
    const bytes = JSON.stringify(event).length * 2;
    if (this.pending.length >= 4096 || this.bytes + bytes > 8 * 1024 * 1024) {
      this.onError(
        new Error("Message store queue full; history repair required"),
      );
      return;
    }
    this.pending.push(event);
    this.bytes += bytes;
    this.draining ??= Promise.resolve().then(() => this.drain());
  }

  private async drain() {
    try {
      const lease = await this.lease;
      while (this.pending.length) {
        const events = this.pending.splice(0, 256);
        if (lease) {
          try {
            await this.call("message_store_put", { lease, events });
          } catch (error) {
            this.onError(error);
          }
        }
        this.bytes = this.pending.reduce(
          (sum, event) => sum + JSON.stringify(event).length * 2,
          0,
        );
      }
    } finally {
      this.draining = null;
    }
  }

  /** Used by shutdown/tests; no timers or additional subscriptions. */
  async flush() {
    await this.draining;
  }
  close() {
    this.closed = true;
  }
}
