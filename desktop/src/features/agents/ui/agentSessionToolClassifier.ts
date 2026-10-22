import type {
  AgentActivityAction,
  AgentActivityDescriptor,
  AgentActivityRenderClass,
  AgentActivityTone,
  TranscriptItem,
} from "./agentSessionTypes";
import {
  formatToolTitle,
  getKuzzToolInfo,
  normalizeToolNameText,
} from "./agentSessionToolCatalog";
import {
  asRecord,
  getToolString,
  getToolStringList,
} from "./agentSessionUtils";
import { type MessageKey, t } from "@/shared/i18n";

type ToolItem = Extract<TranscriptItem, { type: "tool" }>;

export type ToolClassificationInput = {
  title: string;
  toolName: string;
  kuzzToolName: string | null;
  args: Record<string, unknown>;
  result: string;
  isError: boolean;
};

type ToolClassifierProvider = (
  input: ToolClassificationInput,
) => AgentActivityDescriptor | null;

const DEVELOPER_TOOL_BASES = new Set([
  "shell",
  "read_file",
  "view_image",
  "str_replace",
  "todo",
  "stop",
  "postcompact",
]);

const KUZZ_CLI_GROUPS = new Set([
  "messages",
  "channels",
  "dms",
  "reactions",
  "canvas",
  "feed",
  "users",
  "workflows",
  "social",
  "repos",
  "upload",
  "mem",
  "notes",
  "patches",
  "pr",
  "issues",
  "emoji",
  "pack",
]);

const KUZZ_CLI_ADMIN_VERBS = new Set([
  "archive",
  "unarchive",
  "create",
  "delete",
  "remove",
  "add-channel-member",
  "remove-channel-member",
  "set-channel-add-policy",
]);

const KUZZ_CLI_READ_VERBS = new Set([
  "get",
  "list",
  "thread",
  "search",
  "members",
  "runs",
  "notes",
]);

const TOOL_CLASS_LABELS: Record<AgentActivityRenderClass, MessageKey> = {
  message: "agents.toolClass.message",
  "relay-op": "agents.toolClass.relayOp",
  "file-edit": "agents.toolClass.fileEdit",
  "file-read": "agents.toolClass.fileRead",
  "skill-read": "agents.toolClass.skillRead",
  image: "agents.toolClass.image",
  shell: "agents.toolClass.shell",
  status: "agents.toolClass.status",
  thought: "agents.toolClass.thought",
  plan: "agents.toolClass.plan",
  permission: "agents.toolClass.permission",
  error: "agents.toolClass.error",
  generic: "agents.toolClass.generic",
  "raw-rail": "agents.toolClass.rawRail",
  suppressed: "agents.toolClass.suppressed",
};

const providers: ToolClassifierProvider[] = [
  classifyLoadSkillTool,
  classifyDeveloperHarnessTool,
  classifyKuzzTool,
];

export function classifyTool(
  input: ToolClassificationInput,
): AgentActivityDescriptor {
  for (const provider of providers) {
    const descriptor = provider(input);
    if (descriptor) {
      return input.isError || descriptor.renderClass === "error"
        ? {
            ...descriptor,
            renderClass: "error",
            label: descriptor.label.endsWith("failed")
              ? descriptor.label
              : t("agents.toolActivity.failed", { label: descriptor.label }),
          }
        : descriptor;
    }
  }

  return genericDescriptor(input);
}

export function classifyToolItem(item: ToolItem): AgentActivityDescriptor {
  return classifyTool({
    title: item.title,
    toolName: item.toolName,
    kuzzToolName: item.kuzzToolName,
    args: item.args,
    result: item.result,
    isError: item.isError,
  });
}

export function renderClassLabel(renderClass: AgentActivityRenderClass) {
  return t(TOOL_CLASS_LABELS[renderClass]);
}

function classifyLoadSkillTool(
  input: ToolClassificationInput,
): AgentActivityDescriptor | null {
  const isLoadSkill = [input.toolName, input.title, input.kuzzToolName].some(
    (value) => value && normalizeToolNameText(value) === "load_skill",
  );
  if (!isLoadSkill) return null;

  const skillRef = getToolString(input.args, ["name"]);
  const object = skillRef ?? t("agents.toolActivity.objectSkill");
  const isSupportingFile = skillRef?.includes("/") ?? false;

  return {
    renderClass: "skill-read",
    label: isSupportingFile
      ? t("agents.toolActivity.readSkillFile")
      : t("agents.toolActivity.readSkill"),
    preview: skillRef,
    action: { verb: t("agents.toolActivity.verbRead"), object },
    source: "harness",
    groupKey: isSupportingFile ? "skill:load-file" : "skill:load",
  };
}

function classifyDeveloperHarnessTool(
  input: ToolClassificationInput,
): AgentActivityDescriptor | null {
  const kind = resolveDeveloperToolKind(input);
  if (!kind) return null;

  if (kind === "shell") {
    const command = getToolString(input.args, ["command"]);
    const kuzzCli = command ? parseKuzzCliCommand(command) : null;
    if (kuzzCli) {
      return kuzzCli;
    }
    return {
      renderClass: "shell",
      label: t("agents.toolActivity.ranCommand"),
      preview: command,
      action: {
        verb: t("agents.toolActivity.verbRan"),
        object: command ?? t("agents.toolActivity.objectCommand"),
      },
      source: "harness",
      groupKey: "shell:command",
    };
  }

  if (kind === "read_file") {
    const path = getToolString(input.args, ["path"]);
    return {
      renderClass: "file-read",
      label: t("agents.toolActivity.readFile"),
      preview: path,
      action: {
        verb: t("agents.toolActivity.verbRead"),
        object: path ?? t("agents.toolActivity.objectFile"),
      },
      source: "harness",
      groupKey: "read_file",
    };
  }

  if (kind === "view_image") {
    const source = getToolString(input.args, ["source"]);
    return {
      renderClass: "image",
      label: t("agents.toolActivity.viewedImage"),
      preview: source ? basenameOrUrl(source) : null,
      action: {
        verb: t("agents.toolActivity.verbViewed"),
        object: source
          ? basenameOrUrl(source)
          : t("agents.toolActivity.objectImage"),
      },
      source: "harness",
      groupKey: "view_image",
    };
  }

  if (kind === "str_replace") {
    const path = getToolString(input.args, ["path"]);
    return {
      renderClass: "file-edit",
      label: t("agents.toolActivity.editedFile"),
      preview: path,
      action: {
        verb: t("agents.toolActivity.verbEdited"),
        object: path ?? t("agents.toolActivity.objectFile"),
      },
      source: "harness",
      groupKey: "file-edit:str_replace",
    };
  }

  if (kind === "todo") {
    const preview = getTodoPreview(input.args);
    return {
      renderClass: "plan",
      label: t("agents.toolActivity.updatedTodos"),
      preview,
      action: { verb: t("agents.toolActivity.verbUpdated"), object: preview },
      source: "harness",
      groupKey: "plan:todo",
    };
  }

  if (kind === "stop_hook") {
    return {
      renderClass: "suppressed",
      label: t("agents.toolActivity.checkedTodos"),
      preview: null,
      action: {
        verb: t("agents.toolActivity.verbChecked"),
        object: t("agents.toolActivity.objectTodos"),
      },
      source: "harness",
      groupKey: "suppressed:stop-hook",
    };
  }

  if (kind === "post_compact_hook") {
    return {
      renderClass: "status",
      label: t("agents.toolActivity.contextCompacted"),
      preview: null,
      action: {
        verb: t("agents.toolActivity.verbCompacted"),
        object: t("agents.toolActivity.objectContext"),
      },
      source: "harness",
      groupKey: "status:post-compact",
    };
  }

  const preview = genericPreview(input);
  return {
    renderClass: "generic",
    label: t("agents.toolActivity.ranTool"),
    preview,
    action: {
      verb: t("agents.toolActivity.verbRan"),
      object: preview ?? t("agents.toolActivity.objectTool"),
    },
    source: "harness",
    groupKey: "generic:dev-mcp",
  };
}

function classifyKuzzTool(
  input: ToolClassificationInput,
): AgentActivityDescriptor | null {
  const name = [input.kuzzToolName, input.toolName, input.title].find(
    (value) => value && getKuzzToolInfo(value),
  );
  if (!name) return null;

  const info = getKuzzToolInfo(name);
  if (!info) return null;

  const operation = normalizeToolNameText(name);
  const label = formatToolTitle(name, input.title);
  const preview = extractKuzzToolPreview(input.args);
  return {
    renderClass: isKuzzMessageSend(operation) ? "message" : "relay-op",
    label,
    preview,
    action: actionForKuzzOperation(operation, preview, info.tone),
    tone: info.tone,
    operation,
    object: preview,
    source: "mcp",
    groupKey: `kuzz:${operation}`,
  };
}

function genericDescriptor(
  input: ToolClassificationInput,
): AgentActivityDescriptor {
  const preview = genericPreview(input);
  return {
    renderClass: "generic",
    label: t("agents.toolActivity.ranTool"),
    preview,
    action: {
      verb: t("agents.toolActivity.verbRan"),
      object: preview ?? t("agents.toolActivity.objectTool"),
    },
    source: "fallback",
    groupKey: `generic:${normalizeToolNameText(input.toolName || input.title)}`,
  };
}

function resolveDeveloperToolKind(
  input: ToolClassificationInput,
):
  | "shell"
  | "read_file"
  | "view_image"
  | "str_replace"
  | "todo"
  | "stop_hook"
  | "post_compact_hook"
  | "dev_mcp"
  | null {
  for (const value of [input.toolName, input.title, input.kuzzToolName]) {
    const kind = classifyDeveloperToolName(value);
    if (kind) return kind;
  }
  return null;
}

function classifyDeveloperToolName(value: string | null | undefined) {
  if (!value) return null;

  const normalized = normalizeToolNameText(value);
  const base = normalized.replace(/^kuzz_dev_mcp_/, "");

  if (base === "shell" || normalized.endsWith("_shell")) return "shell";
  if (base === "read_file" || normalized.endsWith("_read_file"))
    return "read_file";
  if (base === "view_image" || normalized.endsWith("_view_image"))
    return "view_image";
  if (base === "str_replace" || normalized.endsWith("_str_replace"))
    return "str_replace";
  if (base === "todo") return "todo";
  if (base === "stop") return "stop_hook";
  if (base === "postcompact") return "post_compact_hook";
  if (DEVELOPER_TOOL_BASES.has(base) || normalized.includes("kuzz_dev_mcp")) {
    return "dev_mcp";
  }
  return null;
}

export function parseKuzzCliCommand(
  command: string,
): AgentActivityDescriptor | null {
  const tokens = tokenizeShellCommand(command);
  const range = findKuzzCommand(tokens);
  if (!range) return null;

  const group = tokens[range.groupIndex];
  const verb = tokens[range.verbIndex] ?? "run";
  const operation = `${group}.${verb}`;
  const isSend = group === "messages" && verb === "send";
  const preview = isSend
    ? extractKuzzCliInlineContent(tokens, range)
    : extractKuzzCliObjectPreview(tokens, range);
  const tone = kuzzCliTone(group, verb);
  return {
    renderClass: isSend ? "message" : "relay-op",
    label: titleForKuzzCli(group, verb),
    preview,
    action: actionForKuzzOperation(operation, preview, tone),
    tone,
    operation,
    object: preview,
    source: "shell",
    groupKey: `kuzz-cli:${operation}`,
  };
}

function titleForKuzzCli(group: string, verb: string) {
  if (group === "messages" && verb === "send")
    return t("agents.toolActivity.sendMessage");
  return [group, verb]
    .map((part) =>
      part
        .split(/[-_]+/)
        .filter(Boolean)
        .map((word) => word.charAt(0).toUpperCase() + word.slice(1))
        .join(" "),
    )
    .filter(Boolean)
    .join(" ");
}

function actionForKuzzOperation(
  operation: string,
  object: string | null,
  tone: AgentActivityTone,
): AgentActivityAction {
  const verb = kuzzOperationVerbToken(operation);
  return {
    verb: kuzzOperationVerb(verb, tone),
    object: object ?? kuzzOperationObject(operation),
  };
}

function kuzzOperationVerbToken(operation: string) {
  if (operation.includes(".")) {
    return operation.split(".")[1] ?? "run";
  }
  return operation.split("_")[0] ?? "run";
}

function kuzzOperationVerb(verb: string, tone: AgentActivityTone) {
  if (verb === "add") return t("agents.toolActivity.verbAdded");
  if (verb === "archive") return t("agents.toolActivity.verbArchived");
  if (verb === "create") return t("agents.toolActivity.verbCreated");
  if (verb === "delete") return t("agents.toolActivity.verbDeleted");
  if (verb === "get" || verb === "list" || verb === "members")
    return t("agents.toolActivity.verbRead");
  if (verb === "remove") return t("agents.toolActivity.verbRemoved");
  if (verb === "runs") return t("agents.toolActivity.verbRead");
  if (verb === "search") return t("agents.toolActivity.verbSearched");
  if (verb === "send") return t("agents.toolActivity.verbSent");
  if (verb === "thread") return t("agents.toolActivity.verbRead");
  if (verb === "unarchive") return t("agents.toolActivity.verbUnarchived");
  if (tone === "read") return t("agents.toolActivity.verbRead");
  return t("agents.toolActivity.verbUpdated");
}

function kuzzOperationObject(operation: string) {
  if (isKuzzMessageSend(operation))
    return t("agents.toolActivity.objectMessage");
  if (operation.includes(".")) {
    const [group] = operation.split(".");
    return group ? group.replace(/[-_]+/g, " ") : "Kuzz";
  }
  const object = operation.replace(
    /^(add|approve|archive|create|delete|edit|get|hide|join|leave|list|open|publish|remove|search|send|set|trigger|unarchive|update|vote)_/,
    "",
  );
  return object ? object.replace(/[-_]+/g, " ") : "Kuzz";
}

function kuzzCliTone(group: string, verb: string): AgentActivityTone {
  if (KUZZ_CLI_ADMIN_VERBS.has(verb)) return "admin";
  if (KUZZ_CLI_READ_VERBS.has(verb)) return "read";
  if (group === "feed" && verb === "get") return "read";
  return "write";
}

function extractKuzzCliInlineContent(
  tokens: string[],
  range: KuzzCommandRange,
): string | null {
  const content = getFlagValue(tokens, range.verbIndex + 1, "--content");
  if (!content || content === "-") return null;
  if (content.includes("$") || content.includes("`")) return null;
  return content;
}

function extractKuzzCliObjectPreview(
  tokens: string[],
  range: KuzzCommandRange,
): string | null {
  const flagPreview =
    getFlagValue(tokens, range.verbIndex + 1, "--channel") ??
    getFlagValue(tokens, range.verbIndex + 1, "--event") ??
    getFlagValue(tokens, range.verbIndex + 1, "--query") ??
    getFlagValue(tokens, range.verbIndex + 1, "--name") ??
    getFlagValue(tokens, range.verbIndex + 1, "--file");
  if (flagPreview) return flagPreview;

  const next = tokens[range.verbIndex + 1];
  return next && !isCommandSeparator(next) && !next.startsWith("-")
    ? next
    : null;
}

type KuzzCommandRange = {
  kuzzIndex: number;
  groupIndex: number;
  verbIndex: number;
};

function findKuzzCommand(tokens: string[]): KuzzCommandRange | null {
  for (let i = 0; i < tokens.length; i++) {
    if (!isKuzzExecutable(tokens[i])) continue;

    for (let j = i + 1; j < tokens.length; j++) {
      if (isCommandSeparator(tokens[j])) break;
      if (tokens[j].startsWith("-")) {
        if (
          !tokens[j].includes("=") &&
          tokens[j + 1]?.startsWith("-") === false
        ) {
          j += 1;
        }
        continue;
      }
      if (!KUZZ_CLI_GROUPS.has(tokens[j])) continue;
      const verbIndex = j + 1;
      if (!tokens[verbIndex] || isCommandSeparator(tokens[verbIndex])) {
        return null;
      }
      return { kuzzIndex: i, groupIndex: j, verbIndex };
    }
  }
  return null;
}

export function tokenizeShellCommand(command: string): string[] {
  const tokens: string[] = [];
  let current = "";
  let quote: "'" | '"' | null = null;
  let escaping = false;

  const pushCurrent = () => {
    if (current.length > 0) {
      tokens.push(current);
      current = "";
    }
  };

  for (const char of command) {
    if (escaping) {
      current += char;
      escaping = false;
      continue;
    }
    if (char === "\\" && quote !== "'") {
      escaping = true;
      continue;
    }
    if (quote) {
      if (char === quote) quote = null;
      else current += char;
      continue;
    }
    if (char === "'" || char === '"') {
      quote = char;
      continue;
    }
    if (/\s/.test(char)) {
      pushCurrent();
      continue;
    }
    if (char === "|" || char === ";" || char === "&") {
      pushCurrent();
      tokens.push(char);
      continue;
    }
    current += char;
  }

  if (escaping) current += "\\";
  pushCurrent();
  return tokens;
}

function isKuzzExecutable(token: string) {
  return token === "kuzz" || token.split(/[\\/]/).pop() === "kuzz";
}

function isCommandSeparator(token: string) {
  return token === "|" || token === ";" || token === "&";
}

function getFlagValue(tokens: string[], start: number, flag: string) {
  for (let i = start; i < tokens.length; i++) {
    const token = tokens[i];
    if (isCommandSeparator(token)) return null;
    if (token === flag) {
      return tokens[i + 1] && !isCommandSeparator(tokens[i + 1])
        ? tokens[i + 1]
        : null;
    }
    if (token.startsWith(`${flag}=`)) return token.slice(flag.length + 1);
  }
  return null;
}

function extractKuzzToolPreview(args: Record<string, unknown>): string | null {
  const content = getToolString(args, ["content", "message", "text", "body"]);
  if (content) return content;
  const query = getToolString(args, ["query", "search"]);
  if (query) return query;
  const channelId = getToolString(args, ["channel_id", "channelId"]);
  if (channelId) return channelId;
  const workflowId = getToolString(args, ["workflow_id", "workflowId"]);
  if (workflowId) return workflowId;
  const pubkeys = getToolStringList(args, ["pubkeys", "pubkey"]);
  if (pubkeys.length === 1) return pubkeys[0];
  if (pubkeys.length > 1)
    return t("agents.toolActivity.objectUsers", { count: pubkeys.length });
  return getToolString(args, ["event_id", "eventId", "name"]);
}

function genericPreview(input: ToolClassificationInput): string | null {
  return (
    getToolString(input.args, [
      "command",
      "path",
      "source",
      "query",
      "name",
      "content",
      "message",
    ]) ?? (input.title ? input.title : null)
  );
}

function isKuzzMessageSend(operation: string) {
  return operation === "send_message" || operation === "messages_send";
}

function basenameOrUrl(source: string): string {
  const trimmed = source.trim();
  if (
    trimmed.startsWith("data:image/") ||
    trimmed.startsWith("http://") ||
    trimmed.startsWith("https://")
  ) {
    return trimmed;
  }
  return trimmed.split(/[/\\]/).pop() ?? trimmed;
}

function getTodoPreview(args: Record<string, unknown>): string | null {
  const todos = args.todos;
  if (!Array.isArray(todos)) return t("agents.toolActivity.objectTodoList");
  if (todos.length === 0) return t("agents.toolActivity.objectEmptyList");
  const first = todos[0];
  const firstText =
    first && typeof first === "object"
      ? getToolString(asRecord(first), ["text"])
      : null;
  if (firstText)
    return todos.length > 1 ? `${firstText} (+${todos.length - 1})` : firstText;
  return t("agents.toolActivity.objectItems", { count: todos.length });
}
