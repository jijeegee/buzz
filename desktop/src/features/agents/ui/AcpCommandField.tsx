import type { AcpCommandCandidate } from "@/shared/api/acpCommands";
import { acpCommandPickerState } from "./acpCommandPicker";
import { PersonaDropdownField } from "./PersonaDropdownField";
import { useT } from "@/shared/i18n";

type AcpCommandFieldProps = {
  candidates: AcpCommandCandidate[];
  disabled: boolean;
  onValueChange: (value: string) => void;
  value: string;
};

export function AcpCommandField({
  candidates,
  disabled,
  onValueChange,
  value,
}: AcpCommandFieldProps) {
  const t = useT();
  const picker = acpCommandPickerState(value, candidates);
  return (
    <div className="space-y-1.5">
      <label
        className="text-sm font-medium text-foreground"
        htmlFor="persona-acp-command"
      >
        {t("agents.acpCommand.label")}
      </label>
      <PersonaDropdownField
        disabled={disabled}
        id="persona-acp-command"
        onValueChange={onValueChange}
        options={picker.options}
        placeholder={t("agents.acpCommand.placeholder")}
        value={picker.selectValue}
      />
      <p className="text-xs text-muted-foreground">
        {t("agents.acpCommand.help")}
      </p>
    </div>
  );
}
