import type { AcpCommandCandidate } from "@/shared/api/acpCommands";
import type { PersonaDropdownOption } from "./agentConfigOptions";
import { t } from "@/shared/i18n";

export const DEFAULT_ACP_COMMAND_VALUE = "kuzz-acp";

export function acpCommandPickerState(
  command: string,
  candidates: readonly AcpCommandCandidate[],
): {
  options: PersonaDropdownOption[];
  selectValue: string;
} {
  const options: PersonaDropdownOption[] = [
    { label: t("agents.acpCommand.default"), value: DEFAULT_ACP_COMMAND_VALUE },
    ...candidates.map((candidate) => ({
      label: candidate.command,
      value: candidate.command,
    })),
  ];
  if (command && !options.some((option) => option.value === command)) {
    options.push({
      disabled: true,
      label: t("agents.acpCommand.unavailable", { command }),
      value: command,
    });
  }
  return { options, selectValue: command || DEFAULT_ACP_COMMAND_VALUE };
}
