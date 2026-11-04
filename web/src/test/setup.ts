import { cleanup } from '@testing-library/react'
import { afterEach } from 'vitest'

// Unmount everything a test rendered.
//
// Twenty test files each had their own `afterEach(cleanup)` line, and
// one had the jsdom environment without it. A component left mounted
// keeps its effects running — a fetch stub in one test can answer the
// next test's request — so a missing cleanup shows up as a failure
// somewhere else entirely.
afterEach(cleanup)
