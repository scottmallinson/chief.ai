import js from '@eslint/js';
import prettier from 'eslint-config-prettier';
import reactHooks from 'eslint-plugin-react-hooks';
import reactRefresh from 'eslint-plugin-react-refresh';
import globals from 'globals';
import tseslint from 'typescript-eslint';

export default tseslint.config(
  {
    ignores: ['dist', 'coverage', 'node_modules', 'src-tauri/target', 'src-tauri/gen', '.claude'],
  },

  // Type-aware linting for application and test sources.
  {
    files: ['**/*.{ts,tsx}'],
    extends: [js.configs.recommended, ...tseslint.configs.recommendedTypeChecked],
    languageOptions: {
      ecmaVersion: 2022,
      globals: globals.browser,
      parserOptions: {
        projectService: true,
        tsconfigRootDir: import.meta.dirname,
      },
    },
    plugins: {
      'react-hooks': reactHooks,
      'react-refresh': reactRefresh,
    },
    rules: {
      ...reactHooks.configs.recommended.rules,
      'react-refresh/only-export-components': ['warn', { allowConstantExport: true }],
      '@typescript-eslint/consistent-type-imports': [
        'error',
        { prefer: 'type-imports', fixStyle: 'inline-type-imports' },
      ],
      '@typescript-eslint/no-unused-vars': [
        'error',
        { argsIgnorePattern: '^_', varsIgnorePattern: '^_' },
      ],
      // Chief ships in WKWebView and WebView2, not in the Chromium the tests
      // run in. A regex lookbehind is a *parse* error on an older
      // JavaScriptCore, so one in a module-level `const` takes the whole bundle
      // down before React mounts — a white screen with no clue but
      // "invalid group specifier name" in a console the user has to go and
      // open. It happened on `main` on 2026-08-29.
      //
      // esbuild does not save us: below its safari16.4 target it rewrites the
      // literal to `new RegExp("...")`, which moves the same failure from parse
      // time to module-evaluation time and changes nothing a user can see. This
      // has to be caught in the editor, so it is a lint rule.
      'no-restricted-syntax': [
        'error',
        {
          selector: 'Literal[regex.pattern=/\\(\\?<[=!]/]',
          message:
            'Regex lookbehind is a parse error in the WebViews Chief ships in. Capture the preceding characters and re-emit them instead.',
        },
        {
          selector: "NewExpression[callee.name='RegExp'] > Literal[value=/\\(\\?<[=!]/]",
          message:
            'Regex lookbehind is unsupported in the WebViews Chief ships in, in a string as much as in a literal.',
        },
      ],
      // The privacy mandate: the renderer never talks to the network directly.
      // Inference goes to the local llama.cpp server and integrations are
      // fetched from Rust, both behind Tauri commands.
      'no-restricted-globals': [
        'error',
        {
          name: 'fetch',
          message:
            'This app is on-device only. Network calls belong in the Rust backend, behind a Tauri command.',
        },
      ],
    },
  },

  // shadcn/ui primitives export variant helpers alongside their component.
  {
    files: ['src/components/ui/**/*.tsx'],
    rules: {
      'react-refresh/only-export-components': 'off',
    },
  },

  // Vite/Vitest configs run in Node.
  {
    files: ['vite.config.ts', 'vitest.config.ts'],
    languageOptions: {
      globals: globals.node,
    },
  },

  // Browser tests are Node code that also hands functions to a page to run.
  // There is no React here: Playwright's fixture callbacks take a `use`
  // argument, which the hooks rule would otherwise read as a hook call.
  {
    files: ['e2e/**/*.ts', 'playwright.config.ts'],
    languageOptions: {
      globals: { ...globals.browser, ...globals.node },
    },
    rules: {
      'react-hooks/rules-of-hooks': 'off',
      'react-refresh/only-export-components': 'off',
    },
  },

  // Plain JS tooling configs are not part of a TypeScript project.
  {
    files: ['**/*.{js,mjs,cjs}'],
    extends: [js.configs.recommended, tseslint.configs.disableTypeChecked],
    languageOptions: {
      globals: globals.node,
    },
  },

  // The marketing site is plain browser JS rather than Node tooling, and it
  // holds to the same rule the renderer does: it makes no requests. Everything
  // it decides — which build to offer, which currency to print — it decides
  // from what the browser already knows. Resolving the current release through
  // GitHub's API would work and would also hand every visitor's IP address to
  // GitHub on page load, which is the opposite of what the page claims.
  {
    files: ['website/**/*.js'],
    languageOptions: {
      globals: globals.browser,
    },
    rules: {
      'no-restricted-globals': [
        'error',
        {
          name: 'fetch',
          message:
            'The website makes no requests. The release version is stamped in by scripts/release.mjs instead.',
        },
      ],
    },
  },

  prettier,
);
