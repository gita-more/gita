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
        print!("{}", self.render());
    }

    /// The table as text, so that tests (and anything else) can look
    /// at what would be drawn.
    fn render(&self) -> String {
        let widths = self.column_widths();
        let sep: String = widths
            .iter()
            .map(|w| format!("+{}", "-".repeat(w + 2)))
            .collect::<Vec<_>>()
            .join("")
            + "+\n";
        let mut out = String::new();
        out.push_str(&sep);
        out.push_str(&self.render_row(
            self.headers
                .iter()
                .map(|s| (*s).to_string())
                .collect::<Vec<_>>()
                .as_slice(),
            &widths,
        ));
        out.push_str(&sep);
        for row in &self.rows {
            out.push_str(&self.render_row(row, &widths));
        }
        out.push_str(&sep);
        out
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

    fn render_row(&self, cells: &[String], widths: &[usize]) -> String {
        let parts: Vec<String> = cells
            .iter()
            .zip(widths.iter())
            .map(|(c, w)| format!(" {:<w$} ", c, w = w))
            .collect();
        format!("|{}|\n", parts.join("|"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The columns are as wide as their widest cell, headers included.
    #[test]
    fn the_columns_fit_their_widest_cell() {
        let mut table = Table::new(&["NAME", "DESCRIPTION"]);
        table.add_row(vec!["widgets".to_string(), "a repository".to_string()]);
        table.add_row(vec![
            "a-very-long-name".to_string(),
            String::new(),
        ]);
        assert_eq!(table.column_widths(), vec![16, 12]);
    }

    /// A row shorter than the header row does not shift the columns.
    #[test]
    fn a_short_row_does_not_shift_the_columns() {
        let mut table = Table::new(&["NAME", "DESCRIPTION"]);
        table.add_row(vec!["only-one".to_string()]);
        assert_eq!(table.column_widths(), vec![8, 11]);
    }

    /// The drawn table frames every row to the same width and lines
    /// the cells up inside it.
    #[test]
    fn the_drawn_table_lines_up() {
        let mut table = Table::new(&["NAME"]);
        table.add_row(vec!["a".to_string()]);
        table.add_row(vec!["longer".to_string()]);

        let drawn = table.render();
        let lines: Vec<&str> = drawn.lines().collect();
        // top frame, header, frame, two rows, frame
        assert_eq!(lines.len(), 6, "got:\n{drawn}");
        for line in &lines {
            assert_eq!(line.len(), lines[0].len(), "every line is framed the same: {drawn}");
        }
        assert!(drawn.contains("| NAME   |"), "the header is left-aligned: {drawn}");
        assert!(drawn.contains("| longer |"), "got:\n{drawn}");
    }
}
