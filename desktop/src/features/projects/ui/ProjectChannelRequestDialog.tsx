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
import { formatTtlDuration } from "@/features/channels/lib/ephemeralChannel";
import type { ProjectChannelRequest } from "@/features/projects/projectChannelRequest";
import { useProjectChannelRequests } from "@/features/projects/useProjectChannelRequests";
import { useT } from "@/shared/i18n";

type RequestedChannel = ProjectChannelRequest["request"];

/** Details an owner reviews before approving an agent-requested project channel. */
export function ProjectChannelRequestDetails({
  request,
}: {
  request: RequestedChannel;
}) {
  const t = useT();
  return (
    <dl className="space-y-2 rounded-xl border border-border/60 bg-muted/25 p-3 text-sm">
      <div className="flex gap-3">
        <dt className="w-24 shrink-0 text-muted-foreground">
          {t("projects.createForm.name")}
        </dt>
        <dd className="min-w-0 break-words text-foreground">{request.name}</dd>
      </div>
      {request.description ? (
        <div className="flex gap-3">
          <dt className="w-24 shrink-0 text-muted-foreground">
            {t("projects.createWorkItem.descriptionLabel")}
          </dt>
          <dd className="min-w-0 break-words text-foreground">
            {request.description}
          </dd>
        </div>
      ) : null}
      <div className="flex gap-3">
        <dt className="w-24 shrink-0 text-muted-foreground">
          {t("projects.channelRequest.visibility")}
        </dt>
        <dd className="text-foreground">
          {t("projects.channelRequest.visibilityValue", {
            visibility: request.visibility,
          })}
        </dd>
      </div>
      {request.ttlSeconds ? (
        <div className="flex gap-3">
          <dt className="w-24 shrink-0 text-muted-foreground">
            {t("projects.channelRequest.lifetime")}
          </dt>
          <dd className="min-w-0 break-words text-foreground">
            {t("projects.channelRequest.temporary", {
              duration: formatTtlDuration(request.ttlSeconds),
            })}
          </dd>
        </div>
      ) : null}
      {request.templateName ? (
        <div className="flex gap-3">
          <dt className="w-24 shrink-0 text-muted-foreground">
            {t("projects.createForm.template")}
          </dt>
          <dd className="min-w-0 break-words text-foreground">
            {request.templateName}
          </dd>
        </div>
      ) : null}
    </dl>
  );
}

/** Global owner-review surface for project-channel requests from managed agents. */
export function ProjectChannelRequestDialog() {
  const t = useT();
  const management = useProjectChannelRequests();
  const request = management.request?.request;

  return (
    <AlertDialog
      onOpenChange={(open) => {
        if (!open) management.dismiss();
      }}
      open={request != null}
    >
      <AlertDialogContent data-testid="project-channel-request-dialog">
        <AlertDialogHeader>
          <AlertDialogTitle>
            {t("projects.channelRequest.title")}
          </AlertDialogTitle>
          <AlertDialogDescription>
            {management.project
              ? t("projects.channelRequest.description", {
                  name: management.project.name,
                })
              : t("projects.channelRequest.descriptionUnnamed")}
          </AlertDialogDescription>
        </AlertDialogHeader>
        {request ? <ProjectChannelRequestDetails request={request} /> : null}
        {management.error ? (
          <p className="text-sm text-destructive">{management.error}</p>
        ) : null}
        <AlertDialogFooter>
          <AlertDialogCancel disabled={management.isPending}>
            {t("common.cancel")}
          </AlertDialogCancel>
          <AlertDialogAction
            data-testid="project-channel-request-approve"
            disabled={management.isPending}
            onClick={(event) => {
              event.preventDefault();
              void management.approve();
            }}
          >
            {management.isPending
              ? t("projects.createWorkItem.creating")
              : t("projects.channelRequest.create")}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
