import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import path from "node:path";

// 输出到 dist-vite/，与现有 babel 编译产物 dist/ 完全独立。
// Rust server 通过 /admin/v2/* 路由 mount 这套；老 /admin 路径不变。
// 全部迁完后会做 alias + 删除 dist/ 双链路。
export default defineConfig({
  plugins: [react()],
  // 资源路径前缀：build 后 index.html 引用 /admin/v2/assets/index-*.js，
  // 与 Rust server 的 /admin/v2/assets/:file 路由对齐。
  base: "/admin/v2/",
  build: {
    outDir: "dist-vite",
    emptyOutDir: true,
    rollupOptions: {
      input: path.resolve(__dirname, "index.html"),
    },
  },
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "src"),
    },
  },
  server: {
    port: 5173,
    proxy: {
      // dev mode 下把 API 请求代理到 Rust server
      "/api": "http://127.0.0.1:38090",
      "/admin/login": "http://127.0.0.1:38090",
      "/admin/me": "http://127.0.0.1:38090",
      "/admin/onboarding": "http://127.0.0.1:38090",
      "/admin/events": "http://127.0.0.1:38090",
      "/v1": "http://127.0.0.1:38090",
    },
  },
});
