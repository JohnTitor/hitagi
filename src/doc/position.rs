use lsp_types::Position;

pub fn position_to_offset(text: &str, position: Position) -> Option<usize> {
    let mut line_start = 0usize;
    for (idx, line) in text.split('\n').enumerate() {
        if idx as u32 == position.line {
            let offset_in_line = utf16_col_to_byte_offset(line, position.character);
            return Some(line_start + offset_in_line);
        }
        line_start += line.len() + 1;
    }

    None
}

fn utf16_col_to_byte_offset(line: &str, col: u32) -> usize {
    let mut utf16_units = 0u32;
    for (byte_idx, ch) in line.char_indices() {
        if utf16_units >= col {
            return byte_idx;
        }
        utf16_units += ch.len_utf16() as u32;
        if utf16_units > col {
            return byte_idx;
        }
    }

    line.len()
}

pub fn lsp_position_from_span(line: u32, column: u32) -> Position {
    Position {
        line: line.saturating_sub(1),
        character: column.saturating_sub(1),
    }
}

pub fn offset_to_position(text: &str, offset: usize) -> Option<Position> {
    if offset > text.len() {
        return None;
    }

    let mut line = 0u32;
    let mut col = 0u32;
    for (idx, ch) in text.char_indices() {
        if idx >= offset {
            break;
        }
        if ch == '\n' {
            line = line.saturating_add(1);
            col = 0;
        } else {
            col = col.saturating_add(ch.len_utf16() as u32);
        }
    }

    Some(Position {
        line,
        character: col,
    })
}
