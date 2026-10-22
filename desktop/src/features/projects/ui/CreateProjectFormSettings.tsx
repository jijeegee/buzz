import { ChevronDown, Plus } from "lucide-react";
import * as React from "react";

import { AddDefaultAiRow } from "@/features/agents/ui/AddDefaultAiRow";
import { ChannelPermissionsSettings } from "@/features/channels/ui/ChannelPermissionsSettings";
import { PROJECT_HOME_TEMPLATE_ID } from "@/features/projects/lib/projectHomeTemplate";
import type { CreateProjectFormSettingsState } from "@/features/projects/ui/useCreateProjectFormSettings";
import { TemplateFormDialog } from "@/features/settings/ui/ChannelTemplatesSettingsCard";
import { Button } from "@/shared/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/shared/ui/dropdown-menu";
import { cn } from "@/shared/lib/cn";
import { useT } from "@/shared/i18n";

const NONE_AGENT_VALUE = "__none__";
const NONE_TEAM_VALUE = "__no-team__";
const NO_TEMPLATE_VALUE = "__no-template__";

const SETTINGS_ROW_CLASS =
  "flex min-h-12 items-center justify-between gap-4 rounded-xl border border-input bg-background px-3 py-3";

export function CreateProjectFormSettings({
  addDefaultAi,
  agentPersonaId,
  defaultAi,
  defaultAiJoinsNewChannels,
  disabled,
  handleTemplateChange,
  handleTemplateCreated,
  personas,
  projectVisibility,
  runtimesAvailable,
  setAddDefaultAi,
  setAgentPersonaId,
  setChannelVisibility,
  setProjectVisibility,
  setTeamId,
  teamId,
  teams,
  templateId,
  templates,
  channelVisibility,
}: CreateProjectFormSettingsState & { disabled: boolean }) {
  const t = useT();
  const [isCreateTemplateOpen, setIsCreateTemplateOpen] = React.useState(false);
  // The built-in template's name is app text; user templates keep their own.
  const templateName = (template: { id: string; name: string }) =>
    template.id === PROJECT_HOME_TEMPLATE_ID
      ? t("projects.createForm.projectHomeTemplate")
      : template.name;
  const noneLabel = t("projects.createForm.none");
  const selectedPersona = personas.find(
    (persona) => persona.id === agentPersonaId,
  );
  const selectedTeam = teams.find((team) => team.id === teamId);
  const selectedTemplate = templates.find(
    (template) => template.id === templateId,
  );
  const listingLabel =
    projectVisibility === "unlisted"
      ? t("projects.createForm.unlisted")
      : t("projects.createForm.listed");
  const agentLabel = selectedPersona?.displayName ?? noneLabel;
  const templateLabel = selectedTemplate
    ? templateName(selectedTemplate)
    : noneLabel;
  const teamLabel = selectedTeam?.name ?? noneLabel;
  const agentDisabled = disabled || (!runtimesAvailable && personas.length > 0);
  const teamDisabled = disabled || (!runtimesAvailable && teams.length > 0);

  return (
    <>
      <ChannelPermissionsSettings
        disabled={disabled}
        onVisibilityChange={setChannelVisibility}
        testIdPrefix="create-project-channel"
        visibility={channelVisibility}
      />

      <div className={cn(SETTINGS_ROW_CLASS, disabled && "opacity-50")}>
        <span className="text-sm font-medium text-foreground">
          {t("projects.createForm.template")}
          <span className="ml-1 text-xs font-normal text-muted-foreground/50">
            {t("projects.createForm.templateHint")}
          </span>
        </span>
        <DropdownMenu modal={false}>
          <DropdownMenuTrigger asChild>
            <Button
              aria-label={t("projects.createForm.templateValue", {
                value: templateLabel,
              })}
              className="-mr-2.5 ml-auto h-9 min-w-0 max-w-[60%] justify-end px-2.5 text-right text-sm font-medium text-foreground hover:bg-muted/50"
              data-testid="create-project-template"
              disabled={disabled}
              type="button"
              variant="ghost"
            >
              <span className="truncate text-right">{templateLabel}</span>
              <ChevronDown className="size-4 shrink-0 text-muted-foreground/70" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            <DropdownMenuRadioGroup
              onValueChange={(value) =>
                handleTemplateChange(value === NO_TEMPLATE_VALUE ? "" : value)
              }
              value={templateId || NO_TEMPLATE_VALUE}
            >
              <DropdownMenuRadioItem value={NO_TEMPLATE_VALUE}>
                {noneLabel}
              </DropdownMenuRadioItem>
              {templates.map((template) => (
                <DropdownMenuRadioItem key={template.id} value={template.id}>
                  {templateName(template)}
                </DropdownMenuRadioItem>
              ))}
            </DropdownMenuRadioGroup>
            <DropdownMenuSeparator />
            <DropdownMenuItem onSelect={() => setIsCreateTemplateOpen(true)}>
              <Plus className="size-4" />
              {t("projects.createForm.newTemplate")}
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
        <TemplateFormDialog
          onCreated={handleTemplateCreated}
          onOpenChange={setIsCreateTemplateOpen}
          open={isCreateTemplateOpen}
          template={null}
        />
      </div>

      <div className={cn(SETTINGS_ROW_CLASS, teamDisabled && "opacity-50")}>
        <span className="text-sm font-medium text-foreground">
          {t("projects.createForm.team")}
          <span className="ml-1 text-xs font-normal text-muted-foreground/50">
            {t("projects.createWorkItem.optional")}
          </span>
        </span>
        <DropdownMenu modal={false}>
          <DropdownMenuTrigger asChild>
            <Button
              aria-label={t("projects.createForm.teamValue", {
                value: teamLabel,
              })}
              className="-mr-2.5 ml-auto h-9 min-w-0 max-w-[60%] justify-end px-2.5 text-right text-sm font-medium text-foreground hover:bg-muted/50"
              data-testid="create-project-team"
              disabled={teamDisabled}
              type="button"
              variant="ghost"
            >
              <span className="truncate text-right">{teamLabel}</span>
              <ChevronDown className="size-4 shrink-0 text-muted-foreground/70" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            <DropdownMenuRadioGroup
              onValueChange={(value) =>
                setTeamId(value === NONE_TEAM_VALUE ? "" : value)
              }
              value={teamId || NONE_TEAM_VALUE}
            >
              <DropdownMenuRadioItem value={NONE_TEAM_VALUE}>
                {noneLabel}
              </DropdownMenuRadioItem>
              {teams.map((team) => (
                <DropdownMenuRadioItem key={team.id} value={team.id}>
                  {team.name}
                </DropdownMenuRadioItem>
              ))}
            </DropdownMenuRadioGroup>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>

      <div className={cn(SETTINGS_ROW_CLASS, disabled && "opacity-50")}>
        <span className="text-sm font-medium text-foreground">
          {t("projects.createForm.projectList")}
        </span>
        <DropdownMenu modal={false}>
          <DropdownMenuTrigger asChild>
            <Button
              aria-label={t("projects.createForm.projectListValue", {
                value: listingLabel,
              })}
              className="-mr-2.5 ml-auto h-9 w-fit justify-end px-2.5 text-right text-sm font-medium text-foreground hover:bg-muted/50"
              data-testid="create-project-listing"
              disabled={disabled}
              type="button"
              variant="ghost"
            >
              <span className="text-right">{listingLabel}</span>
              <ChevronDown className="size-4 shrink-0 text-muted-foreground/70" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent
            align="end"
            onCloseAutoFocus={(event) => event.preventDefault()}
            style={{
              minWidth: "var(--radix-dropdown-menu-trigger-width)",
            }}
          >
            <DropdownMenuRadioGroup
              onValueChange={(value) =>
                setProjectVisibility(
                  value === "unlisted" ? "unlisted" : "listed",
                )
              }
              value={projectVisibility}
            >
              <DropdownMenuRadioItem
                data-testid="create-project-listing-option-listed"
                value="listed"
              >
                {t("projects.createForm.listed")}
              </DropdownMenuRadioItem>
              <DropdownMenuRadioItem
                data-testid="create-project-listing-option-unlisted"
                value="unlisted"
              >
                {t("projects.createForm.unlisted")}
              </DropdownMenuRadioItem>
            </DropdownMenuRadioGroup>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>

      <div className={cn(SETTINGS_ROW_CLASS, agentDisabled && "opacity-50")}>
        <span className="text-sm font-medium text-foreground">
          {t("projects.createForm.codingAgent")}
        </span>
        <DropdownMenu modal={false}>
          <DropdownMenuTrigger asChild>
            <Button
              aria-label={t("projects.createForm.codingAgentValue", {
                value: agentLabel,
              })}
              className="-mr-2.5 ml-auto h-9 min-w-0 max-w-[60%] justify-end px-2.5 text-right text-sm font-medium text-foreground hover:bg-muted/50"
              data-testid="create-project-agent"
              disabled={agentDisabled}
              type="button"
              variant="ghost"
            >
              <span className="truncate text-right">{agentLabel}</span>
              <ChevronDown className="size-4 shrink-0 text-muted-foreground/70" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent
            align="end"
            onCloseAutoFocus={(event) => event.preventDefault()}
            style={{
              minWidth: "var(--radix-dropdown-menu-trigger-width)",
            }}
          >
            <DropdownMenuRadioGroup
              onValueChange={(value) =>
                setAgentPersonaId(value === NONE_AGENT_VALUE ? "" : value)
              }
              value={agentPersonaId || NONE_AGENT_VALUE}
            >
              <DropdownMenuRadioItem
                data-testid="create-project-agent-option-none"
                value={NONE_AGENT_VALUE}
              >
                {noneLabel}
              </DropdownMenuRadioItem>
              {personas.map((persona) => (
                <DropdownMenuRadioItem
                  data-testid={`create-project-agent-option-${persona.id}`}
                  key={persona.id}
                  value={persona.id}
                >
                  {persona.displayName}
                </DropdownMenuRadioItem>
              ))}
            </DropdownMenuRadioGroup>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>

      {defaultAiJoinsNewChannels ? (
        <AddDefaultAiRow
          checked={addDefaultAi}
          defaultAi={defaultAi}
          disabled={disabled}
          idPrefix="create-project"
          joinTarget={t("projects.createForm.joinTarget")}
          onCheckedChange={setAddDefaultAi}
        />
      ) : null}
    </>
  );
}
