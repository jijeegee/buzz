import * as React from "react";
import { ChevronDown, Play, Trash2, Upload, Volume2 } from "lucide-react";

import { invokeTauri } from "@/shared/api/tauri";
import { t as translate, useT } from "@/shared/i18n";
import { cn } from "@/shared/lib/cn";
import { Button } from "@/shared/ui/button";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/shared/ui/alert-dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "@/shared/ui/dropdown-menu";
import { Switch } from "@/shared/ui/switch";
import {
  SettingsOptionGroup,
  SettingsOptionGroupList,
  SettingsOptionRow,
} from "./SettingsOptionGroup";
import { SettingsSectionHeader } from "./SettingsSectionHeader";
import {
  selectedVoiceForBackend,
  type VoiceRegistryEntry,
  voiceOptionLabel,
  voicesForBackend,
} from "./voiceSettingsLogic";

export type TtsSettings = {
  version: number;
  agentTextToSpeech: boolean;
  voicePreferences: string[];
};

type TtsVoiceMutation = {
  settings: TtsSettings;
  registry: VoiceRegistryEntry[];
};

export function VoiceSettingsCard() {
  const t = useT();
  const [settings, setSettings] = React.useState<TtsSettings | null>(null);
  const [registry, setRegistry] = React.useState<VoiceRegistryEntry[]>([]);
  const [busy, setBusy] = React.useState(false);
  const [previewing, setPreviewing] = React.useState(false);
  const [deleteCandidate, setDeleteCandidate] =
    React.useState<VoiceRegistryEntry | null>(null);
  const [error, setError] = React.useState<string | null>(null);

  React.useEffect(() => {
    let disposed = false;
    Promise.all([
      invokeTauri<TtsSettings>("get_tts_settings"),
      invokeTauri<VoiceRegistryEntry[]>("list_voice_registry"),
    ])
      .then(([nextSettings, nextRegistry]) => {
        if (!disposed) {
          setSettings(nextSettings);
          setRegistry(nextRegistry);
        }
      })
      .catch((loadError) => {
        if (!disposed) {
          setError(
            loadError instanceof Error
              ? loadError.message
              : translate("settings.voice.loadFailed"),
          );
        }
      });
    return () => {
      disposed = true;
    };
  }, []);

  const saveEnabled = React.useCallback(async (enabled: boolean) => {
    setBusy(true);
    setError(null);
    try {
      const saved = await invokeTauri<TtsSettings>("set_tts_enabled", {
        enabled,
      });
      setSettings(saved);
    } catch (saveError) {
      try {
        const state = await invokeTauri<{ tts_enabled: boolean }>(
          "get_huddle_state",
        );
        setSettings((current) =>
          current
            ? { ...current, agentTextToSpeech: state.tts_enabled }
            : current,
        );
      } catch {
        // Keep the last confirmed state when native reconciliation is
        // unavailable; the visible save error makes the failure explicit.
      }
      setError(
        saveError instanceof Error
          ? saveError.message
          : translate("settings.voice.saveFailed"),
      );
    } finally {
      setBusy(false);
    }
  }, []);

  const savePocketVoice = React.useCallback(async (voiceKey: string) => {
    setBusy(true);
    setError(null);
    try {
      const saved = await invokeTauri<TtsSettings>("set_pocket_voice", {
        voiceKey,
      });
      setSettings(saved);
    } catch (saveError) {
      setError(
        saveError instanceof Error
          ? saveError.message
          : translate("settings.voice.saveFailed"),
      );
    } finally {
      setBusy(false);
    }
  }, []);

  const importPocketVoice = React.useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      const result = await invokeTauri<TtsVoiceMutation | null>(
        "import_pocket_voice",
      );
      if (result) {
        setSettings(result.settings);
        setRegistry(result.registry);
      }
    } catch (importError) {
      setError(
        importError instanceof Error
          ? importError.message
          : translate("settings.voice.importFailed"),
      );
    } finally {
      setBusy(false);
    }
  }, []);

  const deletePocketVoice = React.useCallback(async (voiceKey: string) => {
    setBusy(true);
    setError(null);
    try {
      const result = await invokeTauri<TtsVoiceMutation>(
        "delete_pocket_voice",
        { voiceKey },
      );
      setSettings(result.settings);
      setRegistry(result.registry);
      setDeleteCandidate(null);
    } catch (deleteError) {
      setError(
        deleteError instanceof Error
          ? deleteError.message
          : translate("settings.voice.deleteFailed"),
      );
    } finally {
      setBusy(false);
    }
  }, []);

  const voices = voicesForBackend(registry, "pocket");
  const selectedVoice = selectedVoiceForBackend(
    settings?.voicePreferences ?? [],
    voices,
  );
  const enabled = settings?.agentTextToSpeech ?? true;
  const controlsDisabled = !settings || busy || !enabled;

  return (
    <section className="min-w-0" data-testid="settings-voice">
      <SettingsSectionHeader
        title={t("settings.voice.title")}
        description={t("settings.voice.description")}
      />

      <SettingsOptionGroupList>
        <SettingsOptionGroup title={t("settings.voice.playbackGroup")}>
          <SettingsOptionRow>
            <div className="min-w-0">
              <label
                className="text-sm font-medium"
                htmlFor="agent-text-to-speech-switch"
              >
                {t("settings.voice.agentTts")}
              </label>
              <p
                className="text-sm text-muted-foreground/70"
                data-settings-subcopy
              >
                {t("settings.voice.agentTtsHint")}
              </p>
            </div>
            <Switch
              checked={enabled}
              data-testid="agent-text-to-speech-toggle"
              disabled={!settings || busy}
              id="agent-text-to-speech-switch"
              onCheckedChange={(checked) => {
                if (settings) void saveEnabled(checked);
              }}
            />
          </SettingsOptionRow>
        </SettingsOptionGroup>

        <div
          aria-disabled={!enabled}
          className={cn(
            "transition-opacity",
            !enabled && "pointer-events-none opacity-45",
          )}
          data-testid="pocket-voice-controls"
        >
          <SettingsOptionGroup title={t("settings.voice.voiceGroup")}>
            <SettingsOptionRow>
              <div className="min-w-0">
                <p className="text-sm font-medium">
                  {t("settings.voice.pocketVoice")}
                </p>
                <p
                  className="text-sm text-muted-foreground/70"
                  data-settings-subcopy
                >
                  {t("settings.voice.pocketVoiceHint")}
                </p>
              </div>

              <div className="flex shrink-0 items-center gap-2">
                <DropdownMenu>
                  <DropdownMenuTrigger asChild>
                    <Button
                      aria-label={t("settings.voice.selectorLabel", {
                        name: selectedVoice?.displayName ?? "Mary",
                      })}
                      className="min-w-32 justify-between"
                      data-testid="pocket-voice-selector"
                      disabled={controlsDisabled}
                      variant="outline"
                    >
                      {selectedVoice
                        ? voiceOptionLabel(selectedVoice, voices)
                        : "Mary"}
                      <ChevronDown className="h-4 w-4" />
                    </Button>
                  </DropdownMenuTrigger>
                  <DropdownMenuContent
                    align="end"
                    className="max-h-80 overflow-y-auto"
                  >
                    <DropdownMenuRadioGroup
                      onValueChange={(voiceKey) => {
                        if (settings) void savePocketVoice(voiceKey);
                      }}
                      value={selectedVoice?.key}
                    >
                      {voices.map((voice) => (
                        <DropdownMenuRadioItem
                          key={voice.key}
                          value={voice.key}
                        >
                          {voiceOptionLabel(voice, voices)}
                        </DropdownMenuRadioItem>
                      ))}
                    </DropdownMenuRadioGroup>
                  </DropdownMenuContent>
                </DropdownMenu>
                <Button
                  aria-label={t("settings.voice.previewLabel", {
                    name: selectedVoice?.displayName ?? "Mary",
                  })}
                  data-testid="pocket-voice-preview"
                  disabled={controlsDisabled || previewing || !selectedVoice}
                  onClick={() => {
                    if (!selectedVoice) return;
                    setPreviewing(true);
                    setError(null);
                    void invokeTauri<void>("preview_pocket_voice", {
                      voiceKey: selectedVoice.key,
                    })
                      .catch((previewError) => {
                        setError(
                          previewError instanceof Error
                            ? previewError.message
                            : t("settings.voice.previewFailed"),
                        );
                      })
                      .finally(() => setPreviewing(false));
                  }}
                  size="sm"
                  variant="outline"
                >
                  {previewing ? (
                    <Volume2 className="h-4 w-4 animate-pulse" />
                  ) : (
                    <Play className="h-4 w-4" />
                  )}
                  {t("settings.voice.preview")}
                </Button>
                <Button
                  data-testid="pocket-voice-import"
                  disabled={controlsDisabled}
                  onClick={() => void importPocketVoice()}
                  size="sm"
                  variant="outline"
                >
                  <Upload className="h-4 w-4" />
                  {t("settings.voice.addVoice")}
                </Button>
                {selectedVoice?.key.startsWith("pocket:imported:") && (
                  <Button
                    aria-label={t("settings.voice.deleteLabel", {
                      name: selectedVoice.displayName,
                    })}
                    data-testid="pocket-voice-delete"
                    disabled={controlsDisabled}
                    onClick={() => setDeleteCandidate(selectedVoice)}
                    size="icon"
                    variant="ghost"
                  >
                    <Trash2 className="h-4 w-4" />
                  </Button>
                )}
              </div>
            </SettingsOptionRow>
          </SettingsOptionGroup>
        </div>
      </SettingsOptionGroupList>
      {error && (
        <p
          className="mt-4 text-sm text-destructive"
          data-testid="voice-settings-error"
          role="alert"
        >
          {error}
        </p>
      )}
      <AlertDialog
        onOpenChange={(open) => {
          if (!open) setDeleteCandidate(null);
        }}
        open={deleteCandidate !== null}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              {t("settings.voice.deleteDialogTitle")}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {deleteCandidate
                ? t("settings.voice.deleteDialogBody", {
                    name: deleteCandidate.displayName,
                  })
                : t("settings.voice.deleteDialogBodyGeneric")}
              {selectedVoice?.key === deleteCandidate?.key &&
                ` ${t("settings.voice.deleteDialogFallback")}`}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={busy}>
              {t("common.cancel")}
            </AlertDialogCancel>
            <AlertDialogAction
              className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
              data-testid="confirm-pocket-voice-delete"
              disabled={busy || !deleteCandidate}
              onClick={(event) => {
                event.preventDefault();
                if (deleteCandidate) {
                  void deletePocketVoice(deleteCandidate.key);
                }
              }}
            >
              {t("settings.voice.deleteVoice")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </section>
  );
}
