use std::borrow::Cow;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use anyhow::{anyhow, Result};
use rust_xlsxwriter::{Color, Format, FormatBorder, Workbook, Worksheet};

use crate::models::QueryServerResult;

/// Export query results, picking the format from the file extension:
/// `.csv` writes CSV, anything else writes an Excel workbook.
pub fn export_results(
    results: &[QueryServerResult],
    output_path: &str,
    single_sheet: bool,
) -> Result<()> {
    let is_csv = Path::new(output_path)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("csv"));
    if is_csv {
        export_to_csv(results, output_path)
    } else {
        export_to_excel(results, output_path, single_sheet)
    }
}

/// Export query results to one CSV file (UTF-8, comma-separated, RFC 4180 quoting).
///
/// CSV has no tabs, so all servers go into a single table; when more than one
/// server returned a result set, a leading `Server` column identifies each row's
/// source. NULLs are written as empty fields and values are never clipped.
pub fn export_to_csv(results: &[QueryServerResult], output_path: &str) -> Result<()> {
    let mut writer = BufWriter::new(File::create(output_path)?);
    write_csv(&mut writer, results)?;
    writer.flush()?;
    Ok(())
}

fn write_csv<W: Write>(out: &mut W, results: &[QueryServerResult]) -> Result<()> {
    let with_columns: Vec<&QueryServerResult> =
        results.iter().filter(|r| !r.columns.is_empty()).collect();
    // The same query runs on every server, so the first result's columns are the header.
    let Some(first) = with_columns.first() else {
        return Ok(());
    };
    let include_server = with_columns.len() > 1;

    let mut header: Vec<&str> = Vec::with_capacity(first.columns.len() + 1);
    if include_server {
        header.push("Server");
    }
    header.extend(first.columns.iter().map(String::as_str));
    write_csv_record(out, &header)?;

    for server_result in &with_columns {
        for row in &server_result.rows {
            let mut record: Vec<&str> = Vec::with_capacity(row.len() + 1);
            if include_server {
                record.push(&server_result.server_name);
            }
            record.extend(row.iter().map(|v| v.as_deref().unwrap_or("")));
            write_csv_record(out, &record)?;
        }
    }
    Ok(())
}

fn write_csv_record<W: Write>(out: &mut W, fields: &[&str]) -> Result<()> {
    for (i, field) in fields.iter().enumerate() {
        if i > 0 {
            out.write_all(b",")?;
        }
        out.write_all(csv_escape(field).as_bytes())?;
    }
    out.write_all(b"\n")?;
    Ok(())
}

/// Quote a field when it contains a delimiter, quote, or line break (RFC 4180),
/// doubling any embedded quotes.
fn csv_escape(value: &str) -> Cow<'_, str> {
    if value.contains([',', '"', '\n', '\r']) {
        Cow::Owned(format!("\"{}\"", value.replace('"', "\"\"")))
    } else {
        Cow::Borrowed(value)
    }
}

/// Export query results to Excel.
///
/// When `single_sheet` is false, each server's result set goes on its own worksheet
/// tab. When true, all servers are combined into one worksheet with a leading
/// `Server` column identifying the source of each row.
pub fn export_to_excel(
    results: &[QueryServerResult],
    output_path: &str,
    single_sheet: bool,
) -> Result<()> {
    let mut workbook = Workbook::new();

    let header_format = Format::new()
        .set_bold()
        .set_background_color(Color::RGB(0x8ebb58))
        .set_font_color(Color::White)
        .set_border(FormatBorder::Thin);

    let data_format = Format::new()
        .set_border(FormatBorder::Thin)
        .set_text_wrap();

    if single_sheet {
        export_single_sheet(&mut workbook, results, &header_format, &data_format)?;
    } else {
        export_per_server(&mut workbook, results, &header_format, &data_format)?;
    }

    workbook
        .save(output_path)
        .map_err(|e| anyhow!(e.to_string()))?;
    Ok(())
}

/// Excel caps a single cell at 32,767 characters; longer strings make the writer
/// error out. Clip to keep the export from failing on large CLOB/text values.
const EXCEL_CELL_LIMIT: usize = 32_767;

fn clip_for_excel(value: &str) -> std::borrow::Cow<'_, str> {
    if value.len() <= EXCEL_CELL_LIMIT {
        std::borrow::Cow::Borrowed(value)
    } else {
        std::borrow::Cow::Owned(value.chars().take(EXCEL_CELL_LIMIT).collect())
    }
}

/// Write a value as a number when it parses cleanly, otherwise as a (clipped) string.
fn write_cell(
    worksheet: &mut Worksheet,
    row: u32,
    col: u16,
    value: &str,
    data_format: &Format,
) -> Result<()> {
    if let Ok(n) = value.parse::<f64>() {
        worksheet
            .write_number_with_format(row, col, n, data_format)
            .map_err(|e| anyhow!(e.to_string()))?;
    } else {
        worksheet
            .write_string_with_format(row, col, clip_for_excel(value).as_ref(), data_format)
            .map_err(|e| anyhow!(e.to_string()))?;
    }
    Ok(())
}

fn export_per_server(
    workbook: &mut Workbook,
    results: &[QueryServerResult],
    header_format: &Format,
    data_format: &Format,
) -> Result<()> {
    // Excel worksheet names must be unique; two selected connections can share a
    // display name (they're distinguished by id, not name), so disambiguate on collision.
    let mut used_names: std::collections::HashSet<String> = std::collections::HashSet::new();

    for server_result in results {
        if server_result.columns.is_empty() {
            continue;
        }

        let mut sheet_name: String = server_result.server_name.chars().take(31).collect();
        if !used_names.insert(sheet_name.clone()) {
            let suffix = format!(" ({})", server_result.server_id);
            let base_len = 31usize.saturating_sub(suffix.chars().count());
            sheet_name = server_result.server_name.chars().take(base_len).collect::<String>() + &suffix;
            used_names.insert(sheet_name.clone());
        }

        let worksheet = workbook.add_worksheet();
        worksheet
            .set_name(&sheet_name)
            .map_err(|e| anyhow!(e.to_string()))?;

        for (col, header) in server_result.columns.iter().enumerate() {
            worksheet
                .write_string_with_format(0, col as u16, header, header_format)
                .map_err(|e| anyhow!(e.to_string()))?;
        }

        for (row_idx, row) in server_result.rows.iter().enumerate() {
            for (col_idx, value) in row.iter().enumerate() {
                write_cell(
                    worksheet,
                    row_idx as u32 + 1,
                    col_idx as u16,
                    value.as_deref().unwrap_or(""),
                    data_format,
                )?;
            }
        }

        if !server_result.rows.is_empty() {
            let last_col = (server_result.columns.len() - 1) as u16;
            let last_row = server_result.rows.len() as u32;
            worksheet
                .autofilter(0, 0, last_row, last_col)
                .map_err(|e| anyhow!(e.to_string()))?;
        }

        worksheet.autofit();
        worksheet
            .set_freeze_panes(1, 0)
            .map_err(|e| anyhow!(e.to_string()))?;
    }

    Ok(())
}

fn export_single_sheet(
    workbook: &mut Workbook,
    results: &[QueryServerResult],
    header_format: &Format,
    data_format: &Format,
) -> Result<()> {
    // Use the columns of the first server that returned any, since the same query
    // runs across all servers; a leading "Server" column identifies each row.
    let base_columns = match results.iter().find(|r| !r.columns.is_empty()) {
        Some(r) => &r.columns,
        None => return Ok(()),
    };

    let worksheet = workbook.add_worksheet();
    worksheet
        .set_name("All Servers")
        .map_err(|e| anyhow!(e.to_string()))?;

    worksheet
        .write_string_with_format(0, 0, "Server", header_format)
        .map_err(|e| anyhow!(e.to_string()))?;
    for (col, header) in base_columns.iter().enumerate() {
        worksheet
            .write_string_with_format(0, col as u16 + 1, header, header_format)
            .map_err(|e| anyhow!(e.to_string()))?;
    }

    let mut row: u32 = 1;
    for server_result in results {
        if server_result.columns.is_empty() {
            continue;
        }
        for data_row in &server_result.rows {
            worksheet
                .write_string_with_format(row, 0, &server_result.server_name, data_format)
                .map_err(|e| anyhow!(e.to_string()))?;
            for (col_idx, value) in data_row.iter().enumerate() {
                write_cell(
                    worksheet,
                    row,
                    col_idx as u16 + 1,
                    value.as_deref().unwrap_or(""),
                    data_format,
                )?;
            }
            row += 1;
        }
    }

    if row > 1 {
        let last_col = base_columns.len() as u16; // "Server" + data columns
        worksheet
            .autofilter(0, 0, row - 1, last_col)
            .map_err(|e| anyhow!(e.to_string()))?;
    }

    worksheet.autofit();
    worksheet
        .set_freeze_panes(1, 0)
        .map_err(|e| anyhow!(e.to_string()))?;

    Ok(())
}

#[cfg(test)]
#[path = "tests/query_export.rs"]
mod tests;
