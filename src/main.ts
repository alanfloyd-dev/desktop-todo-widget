import { createApp } from "vue";
import { invoke } from "@tauri-apps/api/core";
import App from "./App.vue";
import "./styles.css";

createApp(App).mount("#app");

if ("__TAURI_INTERNALS__" in window) {
  void invoke("qa_frontend_ready");
  window.addEventListener("pointerdown", () => {
    void invoke("qa_frontend_input", { kind: "pointerdown" });
  }, { once: true });
  window.addEventListener("contextmenu", () => {
    void invoke("qa_frontend_input", { kind: "contextmenu" });
  }, { once: true });
}
