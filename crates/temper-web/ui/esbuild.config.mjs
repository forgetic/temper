// Bundles the two browser entry points to ui/dist/*.js. The Rust server (PR D)
// serves ui/dist as static files; index.html / chat.html load these bundles as
// ES modules.
//
// The TS sources use explicit `.js` import specifiers (TS `bundler` resolution
// with verbatimModuleSyntax — see tsconfig.json), so esbuild is told to resolve
// `./foo.js` against the `.ts` source via the resolveExtensions order plus a
// tiny plugin that maps the `.js` specifier back onto its `.ts` sibling.
//
// This is a plain build script (no test deps); CI runs it as `npm run build`
// purely to prove the bundle compiles.

import { build } from "esbuild";
import { existsSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));

// Rewrite relative `./x.js` imports to the colocated `./x.ts` when only the .ts
// exists. Lets the same source typecheck under TS's bundler resolution AND
// bundle here without duplicating extensions.
const tsExtensionResolver = {
  name: "ts-extension-resolver",
  setup(b) {
    b.onResolve({ filter: /^\.\.?\/.*\.js$/ }, (args) => {
      const tsPath = resolve(args.resolveDir, args.path.replace(/\.js$/, ".ts"));
      if (existsSync(tsPath)) return { path: tsPath };
      return undefined;
    });
  },
};

const common = {
  bundle: true,
  format: "esm",
  target: "es2022",
  platform: "browser",
  minify: true,
  sourcemap: true,
  logLevel: "info",
  plugins: [tsExtensionResolver],
  absWorkingDir: here,
};

await build({
  ...common,
  entryPoints: { board: "src/board-main.ts" },
  outfile: "dist/board.js",
});

await build({
  ...common,
  entryPoints: { chat: "src/chat-main.ts" },
  outfile: "dist/chat.js",
});
