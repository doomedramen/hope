# React + TypeScript + Vite

This template provides a minimal setup to get React working in Vite with HMR and some Oxlint rules.

Currently, two official plugins are available:

- [@vitejs/plugin-react](https://github.com/vitejs/vite-plugin-react/blob/main/packages/plugin-react) uses [Oxc](https://oxc.rs)
- [@vitejs/plugin-react-swc](https://github.com/vitejs/vite-plugin-react/blob/main/packages/plugin-react-swc) uses [SWC](https://swc.rs/)

## React Compiler

The React Compiler is not enabled on this template because of its impact on dev & build performances. To add it, see [this documentation](https://react.dev/learn/react-compiler/installation).

## Expanding the Oxlint configuration

If you are developing a production application, we recommend enabling type-aware lint rules by installing `oxlint-tsgolint` and editing `.oxlintrc.json`:

```json
{
  "$schema": "./node_modules/oxlint/configuration_schema.json",
  "plugins": ["react", "typescript", "oxc"],
  "options": {
    "typeAware": true
  },
  "rules": {
    "react/rules-of-hooks": "error",
    "react/only-export-components": ["warn", { "allowConstantExport": true }]
  }
}
```

See the [Oxlint rules documentation](https://oxc.rs/docs/guide/usage/linter/rules) for the full list of rules and categories.

## Hope branding

Hope stands for **Homelab Operations & Planning Engine**. Its identity uses a
linked “o/p” motif: the outlined lettering in `public/hope-wordmark.svg` appears
in navigation and authentication, while `public/hope-logo.svg` contains the
compact mark for the connection screen and app icons. Both are editable SVGs.
The wordmark becomes white in dark mode; the icon keeps its dark tile for
contrast against both light and dark browser chrome.

After editing the compact mark, regenerate the SVG/ICO/PNG favicons and
home-screen icons with:

```sh
pnpm exec playwright install chromium # only if Chromium is not installed
pnpm generate:icons
```

Commit the generated files in `public/` with the source mark. Icon generation
uses the existing Playwright dependency; normal builds use the committed assets.
`index.html` links the browser icons and `site.webmanifest` names the app and
its 192px and 512px icons.
