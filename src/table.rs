use std::io::{Cursor, Read, Seek};

use calamine::{DataType, Reader, Sheet, SheetType, Xls, Xlsx};
use serde::Deserialize;

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Options {
    pub sheet: Option<String>,
    pub max_rows: Option<usize>,
    pub max_cols: Option<usize>,
}

impl Options {
    pub fn bounds(&self) -> Result<(usize, usize), &'static str> {
        let rows = self.max_rows.unwrap_or(20);
        let cols = self.max_cols.unwrap_or(8);
        if !(1..=40).contains(&rows) || !(1..=12).contains(&cols) {
            return Err("max_rows must be 1 to 40 and max_cols must be 1 to 12");
        }
        if self
            .sheet
            .as_ref()
            .is_some_and(|s| s.is_empty() || s.len() > 128 || s.contains('\0'))
        {
            return Err("invalid worksheet name");
        }
        Ok((rows, cols))
    }
}

#[derive(Clone, Default)]
pub struct Value {
    pub text: String,
    pub numeric: bool,
}

impl Value {
    fn text(s: &str, numeric: bool) -> Self {
        let mut chars = s.chars();
        let mut text: String = chars
            .by_ref()
            .take(128)
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
        if chars.next().is_some() {
            text.push('…');
        }
        Self { text, numeric }
    }

    fn excel(v: &impl DataType) -> Self {
        if let Some(s) = v
            .get_string()
            .or(v.get_datetime_iso())
            .or(v.get_duration_iso())
        {
            return Self::text(s, false);
        }
        let text = if let Some(dt) = v.get_datetime() {
            if dt.is_duration() {
                let seconds = (dt.as_f64() * 86400.0).round() as i64;
                let sign = if seconds < 0 { "-" } else { "" };
                let magnitude = seconds.unsigned_abs();
                format!(
                    "{sign}{}:{:02}:{:02}",
                    magnitude / 3600,
                    (magnitude / 60) % 60,
                    magnitude % 60
                )
            } else {
                let (y, m, d, h, min, s, _) = dt.to_ymd_hms_milli();
                let date = format!("{y:04}-{m:02}-{d:02}");
                if (h, min, s) == (0, 0, 0) {
                    date
                } else {
                    format!("{date} {h:02}:{min:02}:{s:02}")
                }
            }
        } else if let Some(n) = v.get_int() {
            n.to_string()
        } else if let Some(n) = v.get_float() {
            n.to_string()
        } else if let Some(b) = v.get_bool() {
            if b { "TRUE".into() } else { "FALSE".into() }
        } else if let Some(e) = v.get_error() {
            e.to_string()
        } else {
            String::new()
        };
        Self {
            text,
            numeric: v.is_int() || v.is_float() || v.is_datetime(),
        }
    }
}

pub struct Table {
    pub title: String,
    pub rows: Vec<Vec<Value>>,
    pub width: usize,
    pub cropped: bool,
}

impl Table {
    fn new(title: &str) -> Self {
        Self {
            title: Value::text(title, false).text,
            rows: Vec::new(),
            width: 0,
            cropped: false,
        }
    }

    fn put(&mut self, row: usize, col: usize, value: Value) {
        if self.rows.len() <= row {
            self.rows.resize_with(row + 1, Vec::new);
        }
        let line = &mut self.rows[row];
        if line.len() <= col {
            line.resize(col + 1, Value::default());
        }
        line[col] = value;
        self.width = self.width.max(col + 1);
    }
}

pub fn csv(data: &[u8], options: &Options) -> Result<Table, &'static str> {
    if options.sheet.is_some() {
        return Err("CSV files have no worksheets");
    }
    let (rows, cols) = options.bounds()?;
    let text = std::str::from_utf8(data).map_err(|_| "CSV must use UTF-8")?;
    if text
        .bytes()
        .any(|c| c < 32 && !matches!(c, b'\r' | b'\n' | b'\t'))
    {
        return Err("unsupported binary file");
    }
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .from_reader(text.as_bytes());
    let mut table = Table::new("CSV");
    for (r, record) in reader.records().take(rows + 1).enumerate() {
        let record = record.map_err(|_| "invalid CSV")?;
        if r == rows {
            table.cropped = true;
            break;
        }
        table.cropped |= record.len() > cols;
        for (c, field) in record.iter().take(cols).enumerate() {
            table.put(r, c, Value::text(field, field.parse::<f64>().is_ok()));
        }
    }
    Ok(table)
}

fn sheet_name(sheets: &[Sheet], options: &Options) -> Result<String, &'static str> {
    sheets
        .iter()
        .find(|s| {
            s.typ == SheetType::WorkSheet
                && options.sheet.as_ref().is_none_or(|name| *name == s.name)
        })
        .map(|s| s.name.clone())
        .ok_or("worksheet not found")
}

pub fn excel(data: &[u8], options: &Options) -> Result<Table, &'static str> {
    let (rows, cols) = options.bounds()?;
    if data.starts_with(b"PK") {
        zip_limits(Cursor::new(data))?;
        let mut book = Xlsx::new(Cursor::new(data)).map_err(|_| "invalid XLSX workbook")?;
        let name = sheet_name(book.sheets_metadata(), options)?;
        let mut table = Table::new(&name);
        let mut cells = book
            .worksheet_cells_reader(&name)
            .map_err(|_| "invalid worksheet")?;
        while let Some(cell) = cells.next_cell().map_err(|_| "invalid worksheet cell")? {
            let (r, c) = cell.get_position();
            if r as usize >= rows {
                table.cropped = true;
                break;
            }
            if c as usize >= cols {
                table.cropped = true;
                continue;
            }
            table.put(r as usize, c as usize, Value::excel(cell.get_value()));
        }
        Ok(table)
    } else {
        let mut book = Xls::new(Cursor::new(data)).map_err(|_| "invalid XLS workbook")?;
        let name = sheet_name(book.sheets_metadata(), options)?;
        let range = book
            .worksheet_range(&name)
            .map_err(|_| "invalid worksheet")?;
        let mut table = Table::new(&name);
        if let Some((last_r, last_c)) = range.end() {
            table.cropped = last_r as usize >= rows || last_c as usize >= cols;
            for r in 0..rows.min(last_r as usize + 1) {
                for c in 0..cols.min(last_c as usize + 1) {
                    let value = range
                        .get_value((r as u32, c as u32))
                        .map(Value::excel)
                        .unwrap_or_default();
                    table.put(r, c, value);
                }
            }
        }
        Ok(table)
    }
}

fn zip_limits(reader: impl Read + Seek) -> Result<(), &'static str> {
    let mut archive = zip::ZipArchive::new(reader).map_err(|_| "invalid XLSX archive")?;
    if archive.len() > 4096 {
        return Err("too many XLSX parts");
    }
    let mut size = 0_u64;
    for i in 0..archive.len() {
        let file = archive.by_index_raw(i).map_err(|_| "invalid XLSX part")?;
        size = size.saturating_add(file.size());
        if size > 64 * 1024 * 1024 {
            return Err("expanded XLSX exceeds 64 MiB");
        }
    }
    Ok(())
}
