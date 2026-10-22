import { Markdown } from "@/shared/ui/markdown";
import {
  ActivityRow,
  ActivityRowContent,
  ActivityRowLabel,
} from "./ActivityRow";
import { ToolActivity } from "./ToolActivity";
import { formatTranscriptTimestampTitle } from "../agentSessionUtils";
import type { ActivityRenderClassItemProps } from "./types";
import { useT } from "@/shared/i18n";

export function PlanActivity(props: ActivityRenderClassItemProps) {
  const t = useT();
  if (props.item.type === "tool") {
    return <ToolActivity {...props} />;
  }
  if (props.item.type !== "plan") {
    return null;
  }

  if (props.item.isUpdate) {
    return (
      <ActivityRow
        testId="transcript-plan-update-item"
        title={formatTranscriptTimestampTitle(props.item.timestamp)}
      >
        <ActivityRowLabel
          object={<PlanUpdateLabelObject text={props.item.text} />}
          openToneScope="none"
          verb={t("agents.toolActivity.verbUpdated")}
        />
      </ActivityRow>
    );
  }

  return (
    <ActivityRow
      testId="transcript-plan-item"
      title={formatTranscriptTimestampTitle(props.item.timestamp)}
    >
      <ActivityRowLabel
        object={t("agents.transcript.objectPlan")}
        openToneScope="tool"
        verb={t("agents.toolActivity.verbUpdated")}
      />
      <ActivityRowContent className="pt-1 pb-1.5 text-sm leading-5 text-muted-foreground">
        <Markdown
          className="leading-5"
          content={
            props.item.text.trim() || t("agents.transcript.noPlanDetails")
          }
        />
      </ActivityRowContent>
    </ActivityRow>
  );
}

function PlanUpdateLabelObject({ text }: { text: string }) {
  const t = useT();
  return (
    <>
      {t("agents.transcript.objectPlan")}
      {text ? (
        <>
          {" · "}
          <span className="text-foreground">{text}</span>
        </>
      ) : null}
    </>
  );
}
