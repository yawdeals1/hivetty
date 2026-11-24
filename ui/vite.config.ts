import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: { port: 1420, strictPort: true, watch: { ignored: ["**/src-tauri/**"] } },
  // WebView2 is evergreen Chromium. No source maps in the shipped bundle (they tripled its size).
  build: { target: "chrome120", sourcemap: false },
});
