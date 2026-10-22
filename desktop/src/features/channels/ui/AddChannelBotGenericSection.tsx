import { Input } from "@/shared/ui/input";
import { Textarea } from "@/shared/ui/textarea";
import { useT } from "@/shared/i18n";

type AddChannelBotGenericSectionProps = {
  disabled: boolean;
  name: string;
  prompt: string;
  onNameChange: (value: string) => void;
  onPromptChange: (value: string) => void;
};

export function AddChannelBotGenericSection({
  disabled,
  name,
  prompt,
  onNameChange,
  onPromptChange,
}: AddChannelBotGenericSectionProps) {
  const t = useT();
  return (
    <div className="space-y-5 rounded-2xl border border-border/70 bg-card/70 p-4">
      <div>
        <div className="text-sm font-medium">
          {t("channels.addBot.generic.title")}
        </div>
        <p className="text-xs text-muted-foreground">
          {t("channels.addBot.generic.body")}
        </p>
      </div>

      <div className="space-y-1.5">
        <label className="text-sm font-medium" htmlFor="channel-generic-name">
          {t("channels.addBot.generic.name")}
        </label>
        <Input
          autoCapitalize="none"
          autoCorrect="off"
          disabled={disabled}
          id="channel-generic-name"
          onChange={(event) => onNameChange(event.target.value)}
          spellCheck={false}
          value={name}
        />
        <p className="text-xs text-muted-foreground">
          {t("channels.addBot.generic.nameHint")}
        </p>
      </div>

      <div className="space-y-1.5">
        <label className="text-sm font-medium" htmlFor="channel-generic-prompt">
          {t("channels.addBot.generic.prompt")}
        </label>
        <Textarea
          className="min-h-24"
          disabled={disabled}
          id="channel-generic-prompt"
          onChange={(event) => onPromptChange(event.target.value)}
          placeholder={t("channels.addBot.generic.promptPlaceholder")}
          value={prompt}
        />
        <p className="text-xs text-muted-foreground">
          {t("channels.addBot.generic.promptHint")}
        </p>
      </div>
    </div>
  );
}
