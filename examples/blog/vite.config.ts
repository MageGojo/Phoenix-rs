import { defineConfig } from "vite";
import { phoenix } from "@phoenix/vite";

export default defineConfig({
  plugins: [phoenix()],
});
