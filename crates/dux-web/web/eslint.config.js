import js from '@eslint/js'
import globals from 'globals'
import reactHooks from 'eslint-plugin-react-hooks'
import reactRefresh from 'eslint-plugin-react-refresh'
import tseslint from 'typescript-eslint'
import { defineConfig, globalIgnores } from 'eslint/config'

export default defineConfig([
  globalIgnores(['dist']),
  {
    files: ['**/*.{ts,tsx}'],
    extends: [
      js.configs.recommended,
      tseslint.configs.recommended,
      reactHooks.configs.flat.recommended,
      reactRefresh.configs.vite,
    ],
    languageOptions: {
      globals: globals.browser,
    },
  },
  // Vendored shadcn/base-ui primitives in `components/ui/**` intentionally export
  // a component alongside its `cva` variants object (or a context hook such as
  // `useSidebar`). `react-refresh/only-export-components` can't exempt those (they
  // aren't literal constants), and we keep these primitives unforked, so the
  // marginal fast-refresh ergonomics don't justify restructuring every file.
  // Disable just that one rule for this directory.
  {
    files: ['src/components/ui/**/*.{ts,tsx}'],
    rules: {
      'react-refresh/only-export-components': 'off',
    },
  },
])
