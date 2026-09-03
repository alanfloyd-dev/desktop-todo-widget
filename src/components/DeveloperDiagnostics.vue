<script setup lang="ts">
import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import type { DesktopDiagnostics } from "../types";

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
    copyStatus.value = "Copied for GitHub issue";
  } catch (reason) {
    error.value = String(reason);
  }
}
</script>

<template>
  <section class="developer-diagnostics">
    <div class="settings-row developer-heading">
      <div>
        <strong>Support report</strong>
        <small>Privacy-safe · excludes content, paths, location, and URLs</small>
      </div>
      <button type="button" @click="copyDiagnostics">Copy diagnostics</button>
    </div>
    <p v-if="copyStatus" class="copy-status" role="status">{{ copyStatus }}</p>
    <div class="settings-row developer-heading">
      <div>
        <strong>Desktop diagnostics</strong>
        <small>Phase 1 native adapter · read only</small>
      </div>
      <button type="button" @click="refresh">Refresh</button>
    </div>
    <p v-if="error" class="settings-error">{{ error }}</p>
    <dl v-if="report">
      <div><dt>HWND</dt><dd>{{ report.hwnd }}</dd></div>
      <div><dt>Parent</dt><dd>{{ report.parentClass }} · {{ report.parentHwnd }}</dd></div>
      <div><dt>Strategy</dt><dd>{{ report.shellStrategy }}</dd></div>
      <div><dt>Desktop host</dt><dd>{{ report.currentDesktopHwnd }}</dd></div>
      <div><dt>Attachment</dt><dd>{{ report.attachmentValid }} · visible {{ report.isWindowVisible }}</dd></div>
      <div><dt>Style</dt><dd>{{ report.style }} · {{ report.exStyle }}</dd></div>
      <div><dt>Bounds</dt><dd>{{ report.x }},{{ report.y }} · {{ report.width }}×{{ report.height }}</dd></div>
      <div><dt>Recovery</dt><dd>{{ report.recoveryCount }} · {{ report.recoveryReason || "none" }}</dd></div>
      <div><dt>Last attach</dt><dd>{{ report.attach.status }} · {{ report.attach.hwnd }}</dd></div>
      <div><dt>Attach parent</dt><dd>{{ report.attach.parentBefore }} → {{ report.attach.parentTarget }} → {{ report.attach.parentAfter }}</dd></div>
      <div><dt>Attach style</dt><dd>{{ report.attach.styleBefore }} → {{ report.attach.styleAfter }} · ex {{ report.attach.exStyleBefore }} → {{ report.attach.exStyleAfter }}</dd></div>
      <div><dt>Attach bounds</dt><dd>{{ report.attach.boundsBefore }} → {{ report.attach.boundsAfterParent }} → {{ report.attach.boundsFinal }}</dd></div>
      <div><dt>Desktop rect</dt><dd>{{ report.attach.desktopTargetRect }} · client {{ report.attach.desktopClientRect }}</dd></div>
      <div><dt>DPI conversion</dt><dd>logical {{ report.attach.logicalBounds }} · scale {{ report.attach.tauriScaleFactor }} · DPI {{ report.attach.windowDpi }} · physical {{ report.attach.physicalRequested }}</dd></div>
    </dl>
  </section>
</template>
