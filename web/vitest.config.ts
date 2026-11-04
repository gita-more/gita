import { defineConfig } from 'vitest/config'
import react from '@vitejs/plugin-react'

// The web app's tests are mostly component tests: they render a
// component and look at what came out. That needs a DOM.
//
// The configuration used to claim otherwise ("we don't need a DOM
// environment ... not React components") while twenty-one of the
// twenty-six test files carried a `// @vitest-environment jsdom`
// pragma and twenty carried their own `afterEach(cleanup)`. The
// comment described a state of the suite that had stopped being true.
//
// Now the environment is jsdom by default and the setup file does the
// cleanup once. A test file that genuinely wants plain Node — the
// pure-function ones, which are faster there — can still ask for it
// with the same pragma.
export default defineConfig({
  plugins: [react()],
  test: {
    environment: 'jsdom',
    setupFiles: ['./src/test/setup.ts'],
    include: ['src/**/*.test.{ts,tsx}'],
    coverage: {
      provider: 'v8',
      reporter: ['text', 'json-summary', 'lcov'],
      // The floor is a ratchet, not a target. It is deliberately low
      // because `RepoDetail.tsx` — the largest and least covered file
      // in the app — is the subject of work in progress; the number
      // must not go down while that happens.
      thresholds: {
        lines: 60,
        functions: 60,
        branches: 55,
        statements: 60,
      },
      include: ['src/**/*.{ts,tsx}'],
      exclude: [
        'src/**/*.test.{ts,tsx}',
        'src/test/**',
        'src/main.tsx',
        // Pure data catalogues and the icon set: they have no logic to
        // cover, and counting them would make the number meaningless.
        'src/i18n/en.ts',
        'src/i18n/zh.ts',
        'src/i18n/types.ts',
        'src/components/Icons.tsx',
      ],
    },
  },
})
