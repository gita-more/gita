# Gita web

The React + TypeScript single-page app for Gita. It talks to the server
over the same REST API any other client uses (`/api/*`), and is served by
`gita-web` from `web/dist`.

## Layout

```
src/
├── main.tsx          # entry point
├── App.tsx           # routes
├── components/       # one component per screen (Repos, RepoDetail, Issues, …)
├── hooks/            # useAuth, useI18n, …
├── i18n/             # zh / en dictionaries and the translation helpers
└── lib/api.ts        # the typed REST client
```

## Commands

```bash
npm install
npm run dev      # Vite dev server, proxying /api and /git to the backend
npm run build    # type-check (tsc -b) and build into dist/
npm test         # vitest unit tests
npm run lint     # oxlint
```

`npm run build` writes `web/dist`, which the backend serves. A release
build of the server expects that directory next to the binary (or in the
working directory) — see `crates/gita-web`.

## Adding a screen

1. Add the component under `src/components/`.
2. Add the endpoint to `src/lib/api.ts`; every call goes through `api`,
   which attaches the bearer token and throws on a non-2xx response.
3. Register the route in `src/App.tsx`.
4. Add the strings to **both** `src/i18n/zh.ts` and `src/i18n/en.ts` —
   a unit test fails when the two dictionaries disagree.
