import { defineConfig } from "astro/config";

export default defineConfig({
  site: "https://getdux.app",
  output: "static",
  trailingSlash: "ignore",
  build: {
    inlineStylesheets: "auto",
  },
});
