# Spreadsheet Preview

`spreadsheetpreview` converts an Excel workbook or a CSV file into a PNG table.
One Rust executable runs the HTTP server, reads the workbook, and draws the image.
The executable includes its font.

## Build and run

Build with Rust 1.88 or later on Linux or macOS.

```sh
cargo build --release --locked
./target/release/spreadsheetpreview
```

The server listens on `127.0.0.1:3000` by default. Set `BIND_ADDR` to change the address.

## Request a preview

Send the file bytes in the body of `POST /preview`.

```sh
curl --fail-with-body -sS \
  -H 'Content-Type: text/csv' \
  --data-binary @inventory.csv \
  'http://127.0.0.1:3000/preview' \
  -o preview.png
```

For `.xls` or `.xlsx`, use `Content-Type: application/octet-stream` and send the workbook.
Add `?sheet=Sales` to select an Excel worksheet by name.
If you omit `sheet`, the server selects the first worksheet.

The server returns `image/png`. The preview starts at cell A1 and shows up to 20 rows and 8 columns by default.
Set `max_rows` from 1 to 40 and `max_cols` from 1 to 12.
The image shows **Partial sheet** if the limits cut off rows or columns.
The server uses a consistent table style and displays saved formula results when the workbook contains them.

CSV files must use UTF-8 and commas. The upload limit is 8 MiB.
XLSX archives can contain up to 4,096 entries and declare up to 64 MiB of uncompressed data.
The server accepts two requests at a time. Extra requests receive HTTP 503.

## License

The service uses the [MIT License](LICENSE). The bundled DejaVu Sans font uses
the license in [assets/FONT-LICENSE.txt](assets/FONT-LICENSE.txt).
