import assert from "node:assert/strict";
import test from "node:test";
import { registerHooks } from "node:module";
import { WebSession } from "./web-session.ts";

let fetchError;
let releaseFetch;
let delayFetch = false;
globalThis.__gitAuthTest = {
  session: new WebSession({
    baseUrl: "https://relay.test",
    fetch: async (url, init) => {
      if (url.endsWith("/logout")) return new Response(null, { status: 204 });
      return new Response(
        JSON.stringify({
          access: "access",
          expires_in: 3600,
          principal_id: JSON.parse(init.body).login_code,
        }),
      );
    },
    setTimer: () => 0,
    clearTimer: () => {},
  }),
  fetch: async () => {
    if (delayFetch)
      await new Promise((r) => {
        releaseFetch = r;
      });
    if (fetchError) throw fetchError;
  },
};
const modules = {
  "@isomorphic-git/lightning-fs": `export default class FS { constructor(name) { this.name=name; this.promises={stat: async()=>({})}; } }`,
  "isomorphic-git": `export const fetch = (...args) => globalThis.__gitAuthTest.fetch(...args); export const clone=fetch; export const log=fetch,readBlob=fetch,readTree=fetch,resolveRef=fetch;`,
  "isomorphic-git/http/web": `export default {};`,
  "@/shared/auth/session": `export const webSession=globalThis.__gitAuthTest.session; export async function relayAuthorization(){return "Bearer access";}`,
  "@/shared/lib/relay-url": `export const relayHttpBaseUrl=()=>"https://relay.test";`,
};
registerHooks({
  resolve(specifier, context, next) {
    if (modules[specifier])
      return {
        url: `data:text/javascript,${encodeURIComponent(modules[specifier])}`,
        shortCircuit: true,
      };
    return next(specifier, context);
  },
});
const { ensureClone, getFs } = await import(
  "../../features/repos/git-client.ts"
);
const session = globalThis.__gitAuthTest.session;

test("persistent repo clones are isolated by principal, including signed-out reads", async () => {
  await session.completeLogin("alice", "v");
  const alice = getFs("owner", "repo").name;
  await session.signOut();
  const guest = getFs("owner", "repo").name;
  await session.completeLogin("bob", "v");
  const bob = getFs("owner", "repo").name;
  assert.notEqual(alice, guest);
  assert.notEqual(alice, bob);
  assert.notEqual(bob, guest);
  assert.notEqual(getFs("a-b", "c").name, getFs("a", "b-c").name);
});

test("an existing clone cannot turn authorization denial into cached success", async () => {
  fetchError = new Error("HTTP 403 Forbidden");
  await assert.rejects(ensureClone("owner", "repo", "main"), /403/);
  fetchError = undefined;
});

test("a clone completing after account switch is rejected", async () => {
  delayFetch = true;
  const pending = ensureClone("owner", "repo", "main");
  while (!releaseFetch) await new Promise((r) => setImmediate(r));
  await session.signOut();
  releaseFetch();
  await assert.rejects(pending, /account changed/i);
  delayFetch = false;
});
