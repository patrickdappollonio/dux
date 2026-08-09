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
      // `const { omitted, ...rest } = obj` is the standard way to build an
      // object without one of its keys, and the name being bound is the whole
      // point: it exists so the rest element can leave it behind. Turning it
      // into anything else (a delete on a copy, a manual key list) is worse
      // code. `ignoreRestSiblings` exists precisely for this idiom and is
      // narrow: it exempts ONLY a binding that sits next to a rest element in
      // the same destructure, so an ordinary unused variable, parameter or
      // import is still an error everywhere.
      // A leading underscore is this codebase's way of saying a parameter or a
      // caught error is deliberately unused, which a test double relies on
      // constantly (a stub has to accept the arguments the real function takes
      // whether or not it reads them). Naming the options here REPLACES the
      // preset's defaults wholesale rather than extending them, so both
      // patterns have to be restated or the convention silently starts
      // erroring. An unused local VARIABLE is left an error on purpose: unlike
      // a parameter, nothing forces it to exist, so it is far likelier to be a
      // mistake than a marker.
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
