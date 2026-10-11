use std::path::Path;

use super::{dialog_path, display_path};

#[test]
fn wrapped_paths_preserve_all_characters() {
    let input = format!(r"\\?\D:\长路径\{}\样本.txt", "中文abcdef".repeat(15));
    let path = Path::new(&input);
    let wrapped = dialog_path(path);
    assert!(wrapped.contains('\n'));
    assert_eq!(wrapped.replace('\n', ""), display_path(path));
}

#[test]
fn line_breaks_keep_combining_marks_with_their_base() {
    let input = format!("C:\\{}e\u{301}.txt", "a".repeat(36));
    let wrapped = dialog_path(Path::new(&input));
    assert!(wrapped.contains("e\u{301}"));
    assert!(!wrapped.contains("e\n\u{301}"));
    assert_eq!(wrapped.replace('\n', ""), input);
}

#[test]
fn line_breaks_keep_joined_emoji_together() {
    let input = format!("C:\\{}👩\u{200d}💻.txt", "a".repeat(35));
    let wrapped = dialog_path(Path::new(&input));
    assert!(wrapped.contains("👩\u{200d}💻"));
    assert_eq!(wrapped.replace('\n', ""), input);
}
