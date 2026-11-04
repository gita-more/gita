/**
 * Global setup for the Playwright suite.
 *
 * The suite used to run against a server that somebody had started by
 * hand, against a database that accumulated every repository and user
 * any earlier run had created. Two things followed from that:
 *
 * - the suite was not repeatable. A test that saw "the repository
 *   already exists" passed on a fresh database and failed on the
 *   second run, or the other way round;
 * - the suite was not parallel. The tests shared one server and one
 *   namespace, so `workers: 1` was the only setting that did not
 *   produce failures.
 *
 * Now each run gets its own Postgres schema and its own data
 * directory, and the server is started here and stopped at the end of
 * the run. Nothing survives a run: the schema is dropped and the
 * directory is removed.
 *
 * Setting `GITA_URL` keeps the old behaviour: the tests run against
 * that deployment and no server is started. That is useful when
 * pointing the suite at a real deployment on purpose.
 */

import { execFileSync, spawn, type ChildProcess } from 'node:child_process'
import { createServer } from 'node:net'
import { existsSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

import { E2E_BASE, E2E_PORT, externalUrl } from './server-env'

/** The Postgres the tests run against. */
const DB_URL =
  process.env.GITA_TEST_DATABASE_URL ??
  process.env.DATABASE_URL ??
  'postgres://gita:gita@127.0.0.1:55432/gita'

let server: ChildProcess | undefined
let dataDir: string | undefined
let schema: string | undefined

/**
 * Drop the run's schema.
 *
 * The schema holds every user, repository and pipeline the run
 * created. Leaving it behind would make the next run start with a
 * populated database, which is exactly the state this setup exists to
 * avoid.
 */
function dropSchema(name: string) {
  try {
    execFileSync(
      'psql',
      [DB_URL, '-c', `DROP SCHEMA IF EXISTS "${name}" CASCADE`],
      { stdio: 'pipe', timeout: 30_000 },
    )
  } catch (e) {
    // psql may not be installed. The schema is namespaced and harmless
    // if it survives; the next run uses a different name anyway.
    console.warn(`could not drop the schema ${name}: ${e}`)
  }
}

/** Whether something is already listening on the port. */
function isTaken(port: number): Promise<boolean> {
  return new Promise((resolve) => {
    const probe = createServer()
    probe.once('error', () => resolve(true))
    probe.once('listening', () => probe.close(() => resolve(false)))
    probe.listen(port, '127.0.0.1')
  })
}

export default async function globalSetup() {
  const external = externalUrl()
  if (external) {
    // The tests are pointed at a deployment somebody else runs. Nothing
    // is started and nothing is cleaned up — that deployment is not
    // ours to tear down.
    process.env.GITA_URL = external
    console.log(`running the E2E suite against ${external}`)
    return
  }

  // A port that is already in use would mean the suite silently tested
  // somebody else's server. Refuse instead.
  if (await isTaken(E2E_PORT)) {
    throw new Error(
      `port ${E2E_PORT} is already in use, so the E2E suite cannot start its own ` +
        `server. Either stop whatever is listening on it, or set GITA_E2E_PORT to ` +
        `a free port. (If you meant to test that deployment, set GITA_URL instead.)`,
    )
  }

  // A name that no other run uses: the process id keeps concurrent
  // runs apart, and the timestamp keeps a rerun from reusing the
  // leftovers of an earlier one.
  schema = `gita_e2e_${process.pid}_${Date.now()}`
  dataDir = mkdtempSync(join(tmpdir(), 'gita-e2e-'))

  // This file is ESM, so there is no `__dirname`; the path comes from
  // the module's own URL.
  const here = dirname(fileURLToPath(import.meta.url))
  const repoRoot = join(here, '..', '..', '..')
  const configPath = join(dataDir, 'gita-e2e.toml')
  writeFileSync(
    configPath,
    [
      '[server]',
      'host = "127.0.0.1"',
      `port = ${E2E_PORT}`,
      `data_dir = "${dataDir.replace(/\\/g, '/')}"`,
      `database_url = "${DB_URL}"`,
      `database_schema = "${schema}"`,
      'jwt_secret = "e2e-secret"',
      'pages_enabled = true',
      '',
      '[server.vm]',
      'enabled = true',
      'api_key = "e2e-vm-key"',
      '',
    ].join('\n'),
  )

  // The server binary. The CI workflow builds it before the suite
  // runs; locally it is built by `cargo build`.
  const binary = join(repoRoot, 'target', 'debug', 'gita-admin')
  if (!existsSync(binary)) {
    throw new Error(
      `${binary} does not exist. Build it first: cargo build -p gita-admin`,
    )
  }

  server = spawn(binary, ['-c', configPath], {
    cwd: repoRoot,
    stdio: ['ignore', 'pipe', 'pipe'],
    env: { ...process.env, RUST_LOG: process.env.RUST_LOG ?? 'info' },
  })
  server.stdout?.on('data', (d) => {
    if (process.env.GITA_E2E_VERBOSE) process.stdout.write(`[server] ${d}`)
  })
  server.stderr?.on('data', (d) => {
    if (process.env.GITA_E2E_VERBOSE) process.stderr.write(`[server] ${d}`)
  })

  // Wait for the server to answer, rather than sleeping a fixed amount:
  // a fixed sleep is flaky when the machine is loaded and slow when it
  // is not. The first start also runs the migrations, which takes a
  // while, so the deadline is generous.
  const deadline = Date.now() + 120_000
  for (;;) {
    if (server.exitCode !== null) {
      throw new Error(`the server exited with code ${server.exitCode} before it was ready`)
    }
    try {
      const res = await fetch(`${E2E_BASE}/api/health`)
      if (res.ok) break
    } catch {
      // Not up yet.
    }
    if (Date.now() > deadline) {
      throw new Error('the server did not become ready within 120s')
    }
    await new Promise((r) => setTimeout(r, 200))
  }

  // The workers are started after this, and they inherit the
  // environment of the process that runs the setup.
  process.env.GITA_URL = E2E_BASE
  process.env.GITA_E2E_SCHEMA = schema
  process.env.GITA_E2E_DATA_DIR = dataDir
  console.log(`the E2E server is ready at ${E2E_BASE} (schema ${schema})`)

  return async () => {
    // The teardown runs after the last test.
    if (server && server.exitCode === null) {
      server.kill('SIGTERM')
      // Give the server a moment to stop gracefully; it drains its
      // background tasks on the way out.
      const stopBy = Date.now() + 15_000
      while (server.exitCode === null && Date.now() < stopBy) {
        await new Promise((r) => setTimeout(r, 100))
      }
      if (server.exitCode === null) server.kill('SIGKILL')
    }
    if (schema) dropSchema(schema)
    if (dataDir) rmSync(dataDir, { recursive: true, force: true })
  }
}
