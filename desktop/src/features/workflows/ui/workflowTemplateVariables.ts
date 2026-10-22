import { type MessageKey, t } from "@/shared/i18n";
import type {
  ActionType,
  StepFormState,
  TriggerType,
} from "./workflowFormTypes";

export type WorkflowTemplateVariable = {
  description: string;
  group: "Trigger" | "Previous steps";
  value: string;
};

type TemplateVariableDefinition = Omit<
  WorkflowTemplateVariable,
  "description"
> & { descriptionKey: MessageKey };

export type ActiveTemplateToken = {
  end: number;
  query: string;
  start: number;
};

const COMMON_EVENT_VARIABLES: TemplateVariableDefinition[] = [
  {
    value: "trigger.author",
    descriptionKey: "workflows.templateVariable.author",
    group: "Trigger",
  },
  {
    value: "trigger.channel_id",
    descriptionKey: "workflows.templateVariable.groupId",
    group: "Trigger",
  },
  {
    value: "trigger.timestamp",
    descriptionKey: "workflows.templateVariable.timestamp",
    group: "Trigger",
  },
  {
    value: "trigger.message_id",
    descriptionKey: "workflows.templateVariable.message",
    group: "Trigger",
  },
];

const STEP_OUTPUTS: Partial<Record<ActionType, Record<string, MessageKey>>> = {
  delay: { slept_secs: "workflows.templateVariable.sleptSecs" },
  send_message: {
    sent: "workflows.templateVariable.messageSent",
    event_id: "workflows.templateVariable.sentMessageId",
  },
  call_webhook: {
    status: "workflows.templateVariable.httpStatus",
    body: "workflows.templateVariable.httpBody",
  },
  add_reaction: {
    added: "workflows.templateVariable.reactionAdded",
  },
};

function triggerVariableDefinitions(
  triggerType: TriggerType,
): TemplateVariableDefinition[] {
  switch (triggerType) {
    case "message_posted":
      return [
        {
          value: "trigger.text",
          descriptionKey: "workflows.templateVariable.messageText",
          group: "Trigger",
        },
        ...COMMON_EVENT_VARIABLES,
      ];
    case "diff_posted":
      return [
        {
          value: "trigger.text",
          descriptionKey: "workflows.templateVariable.diffText",
          group: "Trigger",
        },
        ...COMMON_EVENT_VARIABLES,
      ];
    case "reaction_added":
      return [
        {
          value: "trigger.emoji",
          descriptionKey: "workflows.templateVariable.reactionEmoji",
          group: "Trigger",
        },
        ...COMMON_EVENT_VARIABLES,
      ];
    case "schedule":
      return COMMON_EVENT_VARIABLES.filter(
        ({ value }) =>
          value === "trigger.channel_id" || value === "trigger.timestamp",
      );
    case "webhook":
      return [
        {
          value: "trigger.channel_id",
          descriptionKey: "workflows.templateVariable.workflowGroupId",
          group: "Trigger",
        },
      ];
  }
}

function triggerVariables(
  triggerType: TriggerType,
): WorkflowTemplateVariable[] {
  return triggerVariableDefinitions(triggerType).map(
    ({ descriptionKey, ...variable }) => ({
      ...variable,
      description: t(descriptionKey),
    }),
  );
}

export function workflowTemplateVariables(
  triggerType: TriggerType,
  previousSteps: StepFormState[],
): WorkflowTemplateVariable[] {
  const priorOutputs = previousSteps.flatMap((step) => {
    const outputs = STEP_OUTPUTS[step.action];
    if (!outputs || !/^[A-Za-z0-9_]+$/.test(step.id)) return [];
    return Object.entries(outputs).map(([field, descriptionKey]) => ({
      value: `steps.${step.id}.output.${field}`,
      description: t(descriptionKey),
      group: "Previous steps" as const,
    }));
  });

  return [...triggerVariables(triggerType), ...priorOutputs];
}

/** Find an unfinished `{{variable` token immediately before the caret. */
export function activeTemplateToken(
  value: string,
  caret: number,
): ActiveTemplateToken | null {
  const beforeCaret = value.slice(0, caret);
  const start = beforeCaret.lastIndexOf("{{");
  if (start < 0) return null;

  const query = beforeCaret.slice(start + 2);
  if (query.includes("}}") || !/^[A-Za-z0-9._]*$/.test(query)) return null;

  // If the caret is inside an existing template, replace the complete token
  // instead of leaving its old suffix behind after a suggestion is chosen.
  const closingBraces = value.indexOf("}}", caret);
  const nestedOpening = value.indexOf("{{", start + 2);
  const end =
    closingBraces >= 0 && (nestedOpening < 0 || closingBraces < nestedOpening)
      ? closingBraces + 2
      : caret;
  return { start, end, query };
}

export function insertTemplateVariable(
  value: string,
  token: ActiveTemplateToken,
  variable: string,
): { caret: number; value: string } {
  const insertion = `{{${variable}}}`;
  return {
    value: `${value.slice(0, token.start)}${insertion}${value.slice(token.end)}`,
    caret: token.start + insertion.length,
  };
}
