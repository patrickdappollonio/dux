import path from "path"
import tailwindcss from "@tailwindcss/vite"
import react from "@vitejs/plugin-react"
import { defineConfig } from "vite"

export default defineConfig({
  base: "./",
  plugins: [react(), tailwindcss()],
  resolve: { alias: { "@": path.resolve(__dirname, "./src") } },
  build: {
    outDir: "dist",
    rolldownOptions: {
      output: {
        // Vite 8 is rolldown-powered. `codeSplitting.groups` is the current
        // (non-deprecated) manual-chunking API. xterm and highlight.js already
        // ride their own async chunks via React.lazy / dynamic import; pulling
        // the React runtime out of the entry keeps the eagerly-loaded app chunk
        // under the 500KB warning limit. `[\\/]` matches the path separator
        // portably as rolldown recommends.
        codeSplitting: {
          groups: [
            {
              name: "react-vendor",
              test: /node_modules[\\/](react|react-dom|scheduler)[\\/]/,
            },
          ],
        },
      },
    },
  },
})
