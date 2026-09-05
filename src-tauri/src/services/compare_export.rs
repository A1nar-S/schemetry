use std::fs::{self, File};
use std::path::{Path, PathBuf};

use anyhow::Result;
use polars::prelude::*;
use rust_xlsxwriter::{Color, Format, FormatBorder, Workbook};

use crate::models::Discrepancy;

fn build_dataframe(discrepancies: &[Discrepancy]) -> Result<DataFrame> {
    let difference: Vec<String> = discrepancies.iter().map(|d| d.difference.clone()).collect();
    let table_name: Vec<String> = discrepancies.iter().map(|d| d.table_name.clone()).collect();
    let column_name: Vec<String> = discrepancies.iter().map(|d| d.column_name.clone()).collect();
    let server_name: Vec<String> = discrepancies.iter().map(|d| d.server_name.clone()).collect();
    let details: Vec<String> = discrepancies.iter().map(|d| d.details.clone()).collect();

    let df = df!(
        "DIFFERENCE" => difference,
        "TABLE_NAME" => table_name,
        "COLUMN_NAME" => column_name,
        "SERVER_NAME" => server_name,
        "DETAILS" => details
    )?;

    Ok(df)
}

pub fn save_discrepancy_reports(
    discrepancies: &[Discrepancy],
    output_folder: &Path,
) -> Result<(PathBuf, PathBuf)> {
    if !output_folder.exists() {
        fs::create_dir_all(output_folder)?;
    }

    let mut df = build_dataframe(discrepancies)?;

    let csv_path = output_folder.join("discrepancy_report.csv");
    let mut csv_file = File::create(&csv_path)?;
    CsvWriter::new(&mut csv_file)
        .with_separator(b';')
        .include_header(true)
        .finish(&mut df)?;

    let xlsx_path = output_folder.join("discrepancy_report.xlsx");
    let mut workbook = Workbook::new();
    let worksheet = workbook.add_worksheet();

    let headers = ["DIFFERENCE", "TABLE_NAME", "COLUMN_NAME", "SERVER_NAME", "DETAILS"];
    for (col, header) in headers.iter().enumerate() {
        worksheet.write_string(0, col as u16, *header)?;
    }

    let bad_format = Format::new()
        .set_background_color(Color::RGB(0xFFC7CE))
        .set_font_color(Color::RGB(0x9C0006))
        .set_border(FormatBorder::Thin);

    let neutral_format = Format::new()
        .set_background_color(Color::RGB(0xFFEB9C))
        .set_font_color(Color::RGB(0x9C5700))
        .set_border(FormatBorder::Thin);

    for (idx, item) in discrepancies.iter().enumerate() {
        let row = (idx + 1) as u32;
        let fmt = match item.difference.as_str() {
            "MISSING" => Some(&bad_format),
            "DIFFERENT" => Some(&neutral_format),
            _ => None,
        };

        if let Some(format) = fmt {
            worksheet.write_string_with_format(row, 0, &item.difference, format)?;
        } else {
            worksheet.write_string(row, 0, &item.difference)?;
        }
        worksheet.write_string(row, 1, &item.table_name)?;
        worksheet.write_string(row, 2, &item.column_name)?;
        worksheet.write_string(row, 3, &item.server_name)?;
        worksheet.write_string(row, 4, &item.details)?;
    }

    worksheet.autofit();
    workbook.save(&xlsx_path)?;

    Ok((xlsx_path, csv_path))
}
