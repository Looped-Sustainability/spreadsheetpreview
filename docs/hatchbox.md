# Deploy on Hatchbox

Docker Engine and the Compose v2 plugin must be available to the `deploy` user on
the application server. See [Hatchbox's Docker Compose guide](https://hatchbox.io/docs/articles/19-deploying-with).

In Hatchbox, connect `Looped-Sustainability/spreadsheetpreview` and select `main`.
Add one process assigned to the `web` role with this command.

```sh
docker compose up --no-build --remove-orphans
```

Keep Compose attached so Hatchbox can supervise it. Set the health check URI to
`/healthz`. Leave `PORT` under Hatchbox's control. Compose maps the assigned port
on `127.0.0.1` to port 8080 in the container. Caddy handles HTTPS.

Hatchbox runs [the repository build hook](../.hatchbox/build). If a custom build
script overrides that hook, set the custom script to `./.hatchbox/build`. The
hook builds the new image before it stops the old container. The service needs
no persistent volume. The container uses its temporary directory for PDF uploads.

Before deploying, set `API_KEY` in the Hatchbox app environment. Generate a
separate key for this app with `openssl rand -hex 32`. Every preview request
must send it as a bearer key. The health check does not require a key.

After Hatchbox deploys the release, run this command with the public app origin
and the full commit SHA.

```sh
bin/check-deploy https://YOUR-APP.hatchboxapp.com FULL_COMMIT_SHA
```

The check requires a healthy response and an exact match for the commit SHA.

## Run the container locally

Set `API_KEY` in your shell first.

```sh
docker compose up --build --detach --wait
bin/check-deploy http://127.0.0.1:3000 development
docker compose down
```
