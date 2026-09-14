import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";

export default defineConfig({
  plugins: [vue()],
  clearScreen: false,
  server: {
    // Dev server binds IPv4 loopback explicitly. Vite's default "localhost"
    // can resolve to ::1 only on Windows, which makes http://127.0.0.1
    // connection-refused and can stall WebView2's dev navigation for tens of
    // seconds. Keep this in sync with build.devUrl in src-tauri/tauri.conf.json.
    host: "127.0.0.1",
    strictPort: true,
    port: 1420,
    watch: {
      ignored: ["**/native-shell-probe.*", "**/src-tauri/target/**"],
    },
  },
});
