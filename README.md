# Spreadsheet Preview

Spreadsheet Preview turns CSV, XLS, and XLSX files into PNG tables with row and
column labels. One Rust executable runs the HTTP server and draws the image with
its bundled font. The service returns the image in the same request and stores
no uploads on disk.

## Build and run

Build with Rust 1.88 or later on Linux or macOS. Set `API_KEY` before starting
the server. To generate a key for local use, run this command.

```sh
export API_KEY="$(openssl rand -hex 32)"
```

Build and start the server.

```sh
cargo build --release --locked
./target/release/spreadsheetpreview
```

The server listens on `127.0.0.1:3000` by default. See the configuration below
to change the address.

`GET /` returns the API instructions below as JSON.

<!-- api-guide:start -->
## Request a preview

Spreadsheet Preview converts CSV, XLS, and XLSX files into PNG tables.
Send the file bytes directly in the body of `POST /preview`. Include
`Authorization: Bearer YOUR_API_KEY`. On success, the server returns HTTP 200
with `Content-Type: image/png`.

Set `API_KEY` in your shell to the server's bearer key. Set `BASE_URL` to the
server URL without a trailing slash. The examples use the local server if you
omit `BASE_URL`.

```sh
printf 'Item,Units\nCartons,120\n' | curl --fail --silent --show-error \
  -H 'Content-Type: text/csv' \
  -H "Authorization: Bearer $API_KEY" \
  --data-binary @- \
  "${BASE_URL:-http://127.0.0.1:3000}/preview" \
  -o preview.png
```

Use `application/octet-stream` for XLS and XLSX files. To preview the Sales
sheet, replace `inventory.xlsx` with the path to your workbook.

```sh
curl --fail --silent --show-error \
  -H 'Content-Type: application/octet-stream' \
  -H "Authorization: Bearer $API_KEY" \
  --data-binary @inventory.xlsx \
  "${BASE_URL:-http://127.0.0.1:3000}/preview?sheet=Sales" \
  -o sales.png
```

| Query parameter | Default | Accepted values |
| --- | --- | --- |
| `sheet` | First worksheet | Exact worksheet name in an Excel workbook. |
| `max_rows` | 20 | 1 to 40. |
| `max_cols` | 8 | 1 to 12. |

The preview starts at A1 and shows **Partial sheet** if the row or column
limit cuts off data. The renderer uses its own table style and displays saved
formula results when the workbook contains them.

CSV files must use UTF-8 and commas. The upload limit is 8 MiB. XLSX archives can
contain up to 4,096 entries and declare up to 64 MiB of uncompressed data. The
server accepts two preview requests at a time.

| HTTP status | Meaning |
| --- | --- |
| 400 | The body is empty or the query parameters are invalid. |
| 401 | The bearer API key is missing or invalid. |
| 408 | The upload took more than 15 seconds. |
| 413 | The server could not read the body within the upload limit. |
| 415 | The server does not accept the request's content type. |
| 422 | The server could not read the file or find the requested worksheet. |
| 503 | Both preview slots are busy. Retry after a delay. |

`GET /healthz` reports `status` and the compiled `revision` without a key.
<!-- api-guide:end -->

## Configuration

| Variable | Default | Purpose |
| --- | --- | --- |
| `API_KEY` | Required | Shared bearer key for preview requests. The server refuses to start without a valid key. |
| `BIND_ADDR` | `127.0.0.1:3000` | HTTP listen address. The Docker image sets it to `0.0.0.0:8080`. |
| `PORT` | `3000` | Host port for Docker Compose. Hatchbox supplies the value during deployment. |
| `BUILD_REVISION` | `development` | Revision recorded at Rust compile time. The Docker build takes it from `REVISION`. |

## Run with Docker and deploy

Set `API_KEY` in your shell before you start Compose.

```sh
docker compose up --build --detach --wait
bin/check-deploy http://127.0.0.1:3000 development
docker compose down
```

The [Hatchbox guide](docs/hatchbox.md) gives the web command, port configuration,
and release check.

## Verify

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo build --locked --release
```

[CI](.github/workflows/ci.yml) builds the Docker image, checks the running
revision, and sends CSV and XLSX requests to the container.

## License

The service uses the [MIT License](LICENSE). The bundled DejaVu Sans font uses
the license in [assets/FONT-LICENSE.txt](assets/FONT-LICENSE.txt).
