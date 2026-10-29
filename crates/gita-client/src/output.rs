//! Human-readable and JSON output formatting.

use anyhow::Result;

/// Print `value` as pretty JSON.
pub fn print_json<T: serde::Serialize>(value: &T) -> Result<()> {
    let s = serde_json::to_string_pretty(value)?;
    println!("{s}");
    Ok(())
}

/// A tiny dependency-free table builder.
#[derive(Default)]
pub struct Table {
    headers: Vec<&'static str>,
    rows: Vec<Vec<String>>,
}

impl Table {
    pub fn new(headers: &[&'static str]) -> Self {
        Self {
            headers: headers.to_vec(),
            ..Default::default()
        }
    }

    pub fn add_row(&mut self, row: Vec<String>) {
        self.rows.push(row);
    }

    pub fn print(&self) {
        let widths = self.column_widths();
        let sep: String = widths
            .iter()
            .map(|w| format!("+{}", "-".repeat(w + 2)))
            .collect::<Vec<_>>()
            .join("")
            + "+\n";
        print!("{sep}");
        self.print_row(
            self.headers
                .iter()
                .map(|s| (*s).to_string())
                .collect::<Vec<_>>()
                .as_slice(),
            &widths,
        );
        print!("{sep}");
        for row in &self.rows {
            self.print_row(row, &widths);
        }
        print!("{sep}");
    }

    fn column_widths(&self) -> Vec<usize> {
        let mut widths: Vec<usize> = self.headers.iter().map(|h| h.len()).collect();
        for row in &self.rows {
            for (i, cell) in row.iter().enumerate() {
                if let Some(w) = widths.get_mut(i) {
                    *w = (*w).max(cell.len());
                }
            }
        }
        widths
    }

    fn print_row(&self, cells: &[String], widths: &[usize]) {
        let parts: Vec<String> = cells
            .iter()
            .zip(widths.iter())
            .map(|(c, w)| format!(" {:<w$} ", c, w = w))
            .collect();
        println!("|{}|", parts.join("|"));
    }
}
