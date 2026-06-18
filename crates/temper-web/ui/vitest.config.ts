import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    // Default to the plain `node` environment so Layer-1 (model) tests pay no
    // DOM cost. Files that need a DOM opt in per-file with the docblock pragma:
    //     // @vitest-environment happy-dom
    // (see test/app.dom.test.ts). This keeps the fast suite fast.
    environment: "node",
    include: ["test/**/*.test.ts"],
  },
});
