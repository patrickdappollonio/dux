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
    rules: {
      // Options named here replace the preset's defaults wholesale rather than
      // extending them, so every pattern the codebase relies on must be
      // restated:
      //   ignoreRestSiblings exempts only a binding next to a rest element in
      //   the same destructure, the `const { omitted, ...rest }` idiom.
      //   `^_` is this codebase's marker for a deliberately unused parameter or
      //   caught error, which test doubles rely on constantly.
      // An unused local variable is left an error: nothing forces it to exist,
      // so it is likelier to be a mistake than a marker.
      '@typescript-eslint/no-unused-vars': [
        'error',
        {
          ignoreRestSiblings: true,
          argsIgnorePattern: '^_',
          caughtErrorsIgnorePattern: '^_',
        },
      ],
    },
  },
  // The vendored shadcn/base-ui primitives in `components/ui/**` export a
  // component alongside its `cva` variants object or a context hook such as
  // `useSidebar`, which `react-refresh/only-export-components` cannot exempt
  // because they are not literal constants, and they stay unforked.
  {
    files: ['src/components/ui/**/*.{ts,tsx}'],
    rules: {
      'react-refresh/only-export-components': 'off',
    },
  },
])
