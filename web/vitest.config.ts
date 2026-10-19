import { defineConfig } from 'vitest/config'
import react from '@vitejs/plugin-react'

// Pure-JS unit tests for the web app. We don't need a DOM environment
// because the tests cover pure functions (translate(), key parity) and
// not React components — keeping node:false (default) means tests run
// in plain Node and stay fast.
export default defineConfig({
  plugins: [react()],
  test: {
    environment: 'node',
    include: ['src/**/*.test.{ts,tsx}'],
  },
})