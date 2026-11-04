/**
 * Where the E2E suite's server lives.
 *
 * The suite used to run against `http://127.0.0.1:3100` — the port the
 * development deployment uses. That was a trap twice over:
 *
 * - if something was already listening on 3100, the suite silently ran
 *   against *that* instead of the server the test started. The symptom
 *   is a login that fails with a 401: the suite's user does not exist
 *   in the other deployment's database, or exists with another
 *   password;
 * - the suite and the development deployment shared one database, so
 *   every run left its users and repositories behind.
 *
 * So the suite uses a port of its own, and refuses to run when
 * something is already listening on it. A loud failure is much better
 * than a suite that quietly tests the wrong server.
 *
 * `GITA_URL` still means "run against that deployment on purpose", and
 * then no server is started at all.
 */

/**
 * The port the suite's own server listens on.
 *
 * Deliberately not 3100: that is the development deployment's port,
 * and a developer running the suite locally usually has it open.
 */
export const E2E_PORT = Number(process.env.GITA_E2E_PORT ?? 3110)

/** The base URL of the suite's own server. */
export const E2E_BASE = `http://127.0.0.1:${E2E_PORT}`

/** Whether the suite should use an externally provided deployment. */
export function externalUrl(): string | undefined {
  return process.env.GITA_URL
}
