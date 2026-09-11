import { createApp } from "vue";
import { invoke } from "@tauri-apps/api/core";
import App from "./App.vue";
import "./styles.css";

createApp(App).mount("#app");

if ("__TAURI_INTERNALS__" in window) {
  void invoke("qa_frontend_ready");
  void invoke<string>("qa_diagnostic_mode").then((mode) => {
    if (mode !== "late") return;
    window.addEventListener("keydown", (event) => {
      const target = event.target as HTMLElement | null;
      if (target?.matches("input, textarea, select, [contenteditable='true']")) return;
      if (event.key.toLowerCase() === "n") {
        void invoke("qa_native_material_control", { action: "attach" });
      } else if (event.key.toLowerCase() === "b") {
        void invoke("qa_native_material_control", { action: "detach" });
      }
    });
  });
  window.addEventListener("pointerdown", () => {
    void invoke("qa_frontend_input", { kind: "pointerdown" });
  }, { once: true });
  window.addEventListener("contextmenu", () => {
    void invoke("qa_frontend_input", { kind: "contextmenu" });
  }, { once: true });
}
