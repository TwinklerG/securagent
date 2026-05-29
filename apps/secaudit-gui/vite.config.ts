import tailwindcss from "@tailwindcss/vite";
import vue from "@vitejs/plugin-vue";
import type { PluginOption } from "vite";
import { defineConfig } from "vite";
import vueDevTools from "vite-plugin-vue-devtools";

const devHost = "127.0.0.1";
const devPort = 1420;
const ignoredBuildOutputs = [
  "**/node_modules/**",
  "**/dist/**",
  "**/test-results/**",
  "**/.cargo-target/**",
  "**/src-tauri/target/**",
];
const devOnlyVueDevTools = serveOnly(vueDevTools());

export default defineConfig({
  plugins: [vue(), devOnlyVueDevTools, tailwindcss()],
  clearScreen: false,
  server: {
    host: devHost,
    port: devPort,
    strictPort: true,
    watch: {
      ignored: ignoredBuildOutputs,
    },
  },
  preview: {
    host: devHost,
    port: 4173,
    strictPort: true,
  },
  envPrefix: ["VITE_", "TAURI_"],
});

function serveOnly(plugin: PluginOption): PluginOption {
  if (!plugin) {
    return plugin;
  }
  if (Array.isArray(plugin)) {
    return plugin.map(serveOnly);
  }
  if (typeof plugin === "object" && "name" in plugin) {
    return {
      ...plugin,
      apply: "serve",
    };
  }
  return plugin;
}
