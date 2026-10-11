use super::raw_argument_tail;

#[test]
fn raw_quotes_and_backslashes_survive() {
    let command: Vec<u16> = "\"C:\\Some Path\\unlinka.exe\"  -- \"C:\\tail\\\\\" \"a b\""
        .encode_utf16()
        .collect();
    assert_eq!(
        String::from_utf16(raw_argument_tail(&command)).unwrap(),
        "-- \"C:\\tail\\\\\" \"a b\""
    );
}

#[test]
fn unpaired_utf16_survives() {
    let mut command: Vec<u16> = "unlinka.exe ".encode_utf16().collect();
    command.extend([0xD800, 32, 0xDC00]);
    assert_eq!(raw_argument_tail(&command), &[0xD800, 32, 0xDC00]);
}
