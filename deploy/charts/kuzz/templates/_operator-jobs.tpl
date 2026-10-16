{{/* Closed rendering foundation for typed Kuzz operator jobs. */}}

{{- define "kuzz.operatorCronJob" -}}
{{- $root := .root -}}
{{- if eq .type "deletionDrain" -}}
{{- $job := $root.Values.operatorJobs.deletionDrain -}}
{{- range $label := list "app.kubernetes.io/name" "app.kubernetes.io/instance" "app.kubernetes.io/component" -}}
{{- if hasKey $job.podLabels $label -}}
{{- fail (printf "operatorJobs.deletionDrain.podLabels may not set chart-owned label %q" $label) -}}
{{- end -}}
{{- end -}}
apiVersion: batch/v1
kind: CronJob
metadata:
  name: {{ include "kuzz.cronJobName" (dict "root" $root "suffix" "deletion-drain") }}
  labels:
    {{- include "kuzz.labels" $root | nindent 4 }}
    app.kubernetes.io/component: deletion-drain
spec:
  schedule: {{ $job.schedule | quote }}
  concurrencyPolicy: Forbid
  successfulJobsHistoryLimit: {{ $job.successfulJobsHistoryLimit }}
  failedJobsHistoryLimit: {{ $job.failedJobsHistoryLimit }}
  jobTemplate:
    spec:
      activeDeadlineSeconds: {{ $job.activeDeadlineSeconds }}
      backoffLimit: 0
      template:
        metadata:
          labels:
            {{- include "kuzz.selectorLabels" $root | nindent 12 }}
            app.kubernetes.io/component: deletion-drain
            {{- with $job.podLabels }}
            {{- toYaml . | nindent 12 }}
            {{- end }}
          annotations:
            {{- toYaml $job.podAnnotations | nindent 12 }}
        spec:
          restartPolicy: Never
          terminationGracePeriodSeconds: {{ $job.terminationGracePeriodSeconds }}
          serviceAccountName: {{ default (include "kuzz.serviceAccountName" $root) $job.serviceAccountName }}
          automountServiceAccountToken: false
          enableServiceLinks: false
          securityContext:
            {{- toYaml $root.Values.relay.securityContext | nindent 12 }}
          {{- with $root.Values.image.pullSecrets }}
          imagePullSecrets:
            {{- toYaml . | nindent 12 }}
          {{- end }}
          containers:
            - name: deletion-drain
              image: {{ include "kuzz.image" $root }}
              imagePullPolicy: {{ $root.Values.image.pullPolicy }}
              securityContext:
                {{- omit $root.Values.relay.containerSecurityContext "readOnlyRootFilesystem" | toYaml | nindent 16 }}
                readOnlyRootFilesystem: true
              command: ["/usr/local/bin/kuzz-admin"]
              args: ["deletions", "drain"]
              env:
                - { name: KUZZ_S3_ENDPOINT, value: {{ required "s3.endpoint is required when operatorJobs.deletionDrain.enabled=true" (include "kuzz.s3Endpoint" $root) | quote }} }
                - { name: KUZZ_S3_BUCKET, value: {{ required "s3.bucket is required when operatorJobs.deletionDrain.enabled=true" $root.Values.s3.bucket | quote }} }
                - { name: KUZZ_S3_REGION, value: {{ $root.Values.s3.region | quote }} }
                - { name: KUZZ_S3_ADDRESSING_STYLE, value: {{ $root.Values.s3.addressingStyle | quote }} }
                - name: DATABASE_URL
                  valueFrom:
                    secretKeyRef:
                      name: {{ include "kuzz.envSecretName" $root }}
                      key: DATABASE_URL
                - name: REDIS_URL
                  valueFrom:
                    secretKeyRef:
                      name: {{ include "kuzz.envSecretName" $root }}
                      key: REDIS_URL
                - name: KUZZ_S3_ACCESS_KEY
                  valueFrom:
                    secretKeyRef:
                      name: {{ include "kuzz.envSecretName" $root }}
                      key: KUZZ_S3_ACCESS_KEY
                      optional: true
                - name: KUZZ_S3_SECRET_KEY
                  valueFrom:
                    secretKeyRef:
                      name: {{ include "kuzz.envSecretName" $root }}
                      key: KUZZ_S3_SECRET_KEY
                      optional: true
              resources:
                {{- toYaml $job.resources | nindent 16 }}
{{- else -}}
{{- fail (printf "unsupported typed operator job %q" .type) -}}
{{- end -}}
{{- end -}}
