use std::{io, path::PathBuf};

use unlinka_i18n::{Locale, Translator};

use super::prompt;

#[test]
fn accepts_only_an_explicit_affirmative() {
    let tr = Translator::new(Locale::EnUs);
    let paths = [PathBuf::from(r"C:\sample.txt")];
    for (answer, expected) in [
        ("y\n", true),
        ("  YeS \r\n", true),
        ("\n", false),
        ("n\n", false),
        ("delete\n", false),
        ("yes please\n", false),
    ] {
        assert_eq!(
            prompt(&paths, &tr, &mut answer.as_bytes(), &mut Vec::new()).unwrap(),
            expected,
            "answer: {answer:?}"
        );
    }
}

#[test]
fn input_ending_before_an_answer_is_a_confirmation_error() {
    let error = prompt(
        &[PathBuf::from(r"C:\sample.txt")],
        &Translator::new(Locale::EnUs),
        &mut io::empty(),
        &mut Vec::new(),
    )
    .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
}

#[test]
fn shows_complete_local_and_network_paths_before_confirmation() {
    let local = format!(r"C:\长路径\{}\样本.txt", "中文abcdef".repeat(15));
    let network = r"\\server\share\sample.txt";
    let paths = [
        PathBuf::from(format!(r"\\?\{local}")),
        PathBuf::from(r"\\?\UNC\server\share\sample.txt"),
    ];
    let mut output = Vec::new();
    assert!(
        !prompt(
            &paths,
            &Translator::new(Locale::ZhCn),
            &mut "\n".as_bytes(),
            &mut output,
        )
        .unwrap()
    );
    let text = String::from_utf8(output).unwrap();
    assert!(text.contains(&local));
    assert!(text.contains(network));
    assert!(text.ends_with("[y/N] "));
}

#[test]
fn output_failure_prevents_reading_an_affirmative_answer() {
    let mut answer = "yes\n".as_bytes();
    let error = prompt(
        &[PathBuf::from(r"C:\sample.txt")],
        &Translator::new(Locale::EnUs),
        &mut answer,
        &mut io::Cursor::new([0u8; 0]),
    )
    .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::WriteZero);
    assert_eq!(answer, b"yes\n");
}
