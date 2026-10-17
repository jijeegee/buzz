# Changelog

Kuzz is a fork of [Buzz](https://github.com/block/buzz), taken on 2026-10-02
after upstream desktop v0.5.26. Upstream history up to that point lives in the
Buzz repository. This file records Kuzz changes only. Add one line per merged
change, newest first.

## 2026-10-17

- Remaining internal names: local and test database credentials, dev volumes
  and the dev MCP crate use `kuzz`. Upstream changelogs removed; this file starts
  the Kuzz history.
- Mobile: open channel and thread windows at a message and page toward newer
  messages (#44).

## 2026-10-16

- Renamed Buzz to Kuzz across apps, crates, copy and the relay binary (#43).
- Mobile: own messages are drawn on the right from the first frame (#42).
- Relay: page channel and thread windows toward newer messages (#40).
- Mobile: open Inbox and notification threads without loading the channel (#38).
- Mobile: build threads from the bottom like channels (#37).
- Kuzz logo, app icons, brand blue and light-blue default theme (#35).
- Mobile: inbox summary, two-wave preload, retained threads (#28).
- Goals: icon-only thread links, notes behind a note button.
- Desktop: upward scrolling is no longer pulled back to the bottom; thread
  history loads anchored on a reply.

## 2026-10-15

- Automatic mentions on the channel main timeline; added and removed agents are
  saved on the device (#36).
- Each signed-in account's agents are listed on its own device (#34).
- Desktop: developer terms hidden, Admin gated, locked to the Kuzz server,
  community controls removed (#33).
- Claude Code agents receive room goals through the system prompt.
- Onboarding: signed-in users can skip the optional profile step.

## 2026-10-14

- Desktop (Windows): closing the window keeps Kuzz in the tray with agents
  running (#30); New Channel removed from the tray menu (#31).
- Device robots v2 accepted by the relay; retry loop fixed.
- Kuzz Agent: replies through Kuzz tools, effort levels from the subscription
  model, bounded effort retries, OpenAI prompt-cache session key.
- Per-room goals switch: new rooms off, room notice, goals restated after
  compaction.

## 2026-10-13

- Kuzz Agent can use an existing ChatGPT subscription.
- Mobile: Channels and Threads rows open at the newest message (#29).
- Device robot palette: gradients, assigned in device order.

## 2026-10-12

- Multi-account desktop sign-in (#22).
- Inbox summary in one relay request with live row updates (#23); rooms behind
  the top inbox rows are preloaded (#24).
- Desktop shows Kuzz instead of Buzz in user-facing copy (#25); Block-only
  Builderlab, updates and upstream links hidden (#26).
- Release builds prefer bundled sidecars (#15).
- Observer telemetry tiers reverted (#20, #21).

## 2026-10-11

- Goals: goal chip in the thread title bar (#12), links to other goals' threads
  (#13), goal context sent once and then only on change (#14), goals linked to a
  single message (#16).
- Agents get only the base-prompt sections they need (#17).
- Relay: confirmed signup for key-backup logins (#18).
- Desktop: sidebar Working rows show the thread (#19).
- Stable per-install device ids.
- Chat bubbles with the time beside the bubble, desktop and mobile.

## 2026-10-10

- Threads stay one level deep (#3).
- Agent text settings apply to running agents without restarts (#8).
- Goals: whole tree in threads, edit only the thread's branch (#10); agents start
  and own goals for their thread work (#11).
- Desktop: timeline as chat bubbles, gray centered notices, floating thread panel.
- Owner-only agent host devices with per-device robots; robots on mention chips.
- Light, dark or system theme kept per device.
- Desktop: Ctrl+R / F5 no longer reload installed builds.

## 2026-10-09

- Goals: live goal context without restarts, thread goal panel, reviewed agent
  rules (#4).
- Observer telemetry tiers (#2, #5, #6, #7; reverted on 2026-10-12).
- Inbox: panel beside the chat, in-place navigation, Channels + Threads list,
  filter memory.

## 2026-10-08

- Experiments settings page and task-thread auto-open levels.
- Device robots, renamable devices and owner badges.
- Activity filter for Channels + Threads; automatic thread agent mentions on
  mobile; goal layers (experiment, off by default).

## 2026-10-07

- Sign-in recovery; mobile accounts kept on logout (#1).
- Shared thread names, task threads, reply in thread and quote, chat-style
  replies, context gauge and compact control, mobile TTS engine picker.

## 2026-10-03 – 2026-10-06

- Centralized token auth for desktop, web, CLI, huddles, media and invites;
  Google sign-in on desktop and mobile with same-key recovery.
- Onboarding connects to the default community; unified Chats with personal
  pins.
- Default AI: one managed agent marked as default, launched in dispatcher mode
  and added to new channels.
- Thinking effort chosen wherever a model is chosen; Settings › Models with
  providers and task models.
