import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri 期望固定端口；构建产物输出到 dist/（tauri.conf frontendDist）。
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    // 用 5180 避开本机其它 Vite 项目（5173/5174 常被占用）
    port: 5180,
    strictPort: true,
  },
  build: {
    target: "es2021",
    outDir: "dist",
  },
});
