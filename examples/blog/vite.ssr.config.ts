import { defineConfig } from "vite";

export default defineConfig({
  publicDir: false,
  build: {
    emptyOutDir: true,
    outDir: "public/ssr",
    ssr: "views/renderer.tsx",
  },
});
