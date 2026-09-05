use anyhow::{anyhow, Result};
use rust_xlsxwriter::{Color, Format, FormatBorder, Workbook, Worksheet};

use crate::models::QueryServerResult;

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
    for server_result in results {
        if server_result.columns.is_empty() {
            continue;
        }

        let sheet_name: String = server_result.server_name.chars().take(31).collect();

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
