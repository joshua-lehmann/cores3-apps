import { defineConfig } from "vite";

export default defineConfig({
  base: process.env.VITE_GITHUB_PAGES === "true" ? "/cores3-apps/" : "/",
  build: {
    outDir: "dist/client",
    emptyOutDir: true,
  },
});
