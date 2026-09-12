<script setup lang="ts">
import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { useI18n } from "../i18n";
import type { DesktopDiagnostics } from "../types";

const { t } = useI18n();
const report = ref<DesktopDiagnostics | null>(null);
const error = ref("");
const copyStatus = ref("");

async function refresh() {
  error.value = "";
  try {
    report.value = await invoke<DesktopDiagnostics>("window_diagnostics");
  } catch (reason) {
    error.value = String(reason);
  }
}

async function copyDiagnostics() {
  error.value = "";
  copyStatus.value = "";
  try {
    const text = await invoke<string>("copyable_diagnostics");
    await navigator.clipboard.writeText(text);
    copyStatus.value = t("settings.developer.copied");
  } catch (reason) {
    error.value = String(reason);
  }
}
</script>

<template>
  <section class="developer-diagnostics">
    <div class="settings-row developer-heading">
      <div>
        <strong>{{ t("settings.developer.supportReport") }}</strong>
        <small>{{ t("settings.developer.supportReportHint") }}</small>
      </div>
      <button type="button" @click="copyDiagnostics">{{ t("settings.developer.copyDiagnostics") }}</button>
    </div>
    <p v-if="copyStatus" class="copy-status" role="status">{{ copyStatus }}</p>
    <div class="settings-row developer-heading">
      <div>
        <strong>{{ t("settings.developer.desktopDiagnostics") }}</strong>
        <small>{{ t("settings.developer.desktopDiagnosticsHint") }}</small>
      </div>
      <button type="button" @click="refresh">{{ t("settings.developer.refresh") }}</button>
    </div>
    <p v-if="error" class="settings-error">{{ error }}</p>
    <dl v-if="report">
      <div><dt>{{ t("settings.developer.hwnd") }}</dt><dd>{{ report.hwnd }}</dd></div>
      <div><dt>{{ t("settings.developer.parent") }}</dt><dd>{{ report.parentClass }} · {{ report.parentHwnd }}</dd></div>
      <div><dt>{{ t("settings.developer.strategy") }}</dt><dd>{{ report.shellStrategy }}</dd></div>
      <div><dt>{{ t("settings.developer.desktopHost") }}</dt><dd>{{ report.currentDesktopHwnd }}</dd></div>
      <div>
        <dt>{{ t("settings.developer.attachment") }}</dt>
        <dd>
          {{
            t("settings.developer.attachmentValue", {
              valid: String(report.attachmentValid),
              visible: String(report.isWindowVisible),
            })
          }}
        </dd>
      </div>
      <div><dt>{{ t("settings.developer.style") }}</dt><dd>{{ report.style }} · {{ report.exStyle }}</dd></div>
      <div><dt>{{ t("settings.developer.bounds") }}</dt><dd>{{ report.x }},{{ report.y }} · {{ report.width }}×{{ report.height }}</dd></div>
      <div>
        <dt>{{ t("settings.developer.recovery") }}</dt>
        <dd>{{ report.recoveryCount }} · {{ report.recoveryReason || t("settings.developer.none") }}</dd>
      </div>
      <div><dt>{{ t("settings.developer.lastAttach") }}</dt><dd>{{ report.attach.status }} · {{ report.attach.hwnd }}</dd></div>
      <div><dt>{{ t("settings.developer.attachParent") }}</dt><dd>{{ report.attach.parentBefore }} → {{ report.attach.parentTarget }} → {{ report.attach.parentAfter }}</dd></div>
      <div>
        <dt>{{ t("settings.developer.attachStyle") }}</dt>
        <dd>
          {{
            t("settings.developer.attachStyleValue", {
              before: report.attach.styleBefore,
              after: report.attach.styleAfter,
              exBefore: report.attach.exStyleBefore,
              exAfter: report.attach.exStyleAfter,
            })
          }}
        </dd>
      </div>
      <div>
        <dt>{{ t("settings.developer.attachBounds") }}</dt>
        <dd>
          {{
            t("settings.developer.attachBoundsValue", {
              before: report.attach.boundsBefore,
              afterParent: report.attach.boundsAfterParent,
              final: report.attach.boundsFinal,
            })
          }}
        </dd>
      </div>
      <div>
        <dt>{{ t("settings.developer.desktopRect") }}</dt>
        <dd>
          {{
            t("settings.developer.desktopRectValue", {
              target: report.attach.desktopTargetRect,
              client: report.attach.desktopClientRect,
            })
          }}
        </dd>
      </div>
      <div>
        <dt>{{ t("settings.developer.dpiConversion") }}</dt>
        <dd>
          {{
            t("settings.developer.dpiConversionValue", {
              logical: report.attach.logicalBounds,
              scale: report.attach.tauriScaleFactor,
              dpi: report.attach.windowDpi,
              physical: report.attach.physicalRequested,
            })
          }}
        </dd>
      </div>
    </dl>
  </section>
</template>
