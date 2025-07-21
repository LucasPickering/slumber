use std::iter;

/// Print rows in a table
pub fn print_table<const N: usize>(header: [&str; N], rows: &[[String; N]]) {
    // For each column, find the largest width of any cell
    let mut widths = [0; N];
    for column in 0..N {
        widths[column] = iter::once(header[column].len())
            .chain(rows.iter().map(|row| row[column].len()))
            .max()
            .unwrap_or_default()
            + 1; // Min width, for spacing
    }

    for (header, width) in header.into_iter().zip(widths.iter()) {
        print!("{header:<width$}");
    }
    println!();
    for row in rows {
        for (cell, width) in row.iter().zip(widths) {
            print!("{cell:<width$}");
        }
        println!();
    }
}
