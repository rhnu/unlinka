use super::*;

#[test]
fn ico_preserves_transparent_colors_and_native_sizes() {
    let tree = usvg::Tree::from_data(
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"><rect width="8" height="8" fill="#ff0000" fill-opacity="0.5"/></svg>"##,
        &usvg::Options::default(),
    ).unwrap();
    let directory = ico::IconDir::read(io::Cursor::new(icon(&tree).unwrap())).unwrap();
    // Small taskbar icons must not be downsampled from a single large frame.
    assert!(directory.entries().iter().any(|entry| entry.width() == 16));
    assert!(directory.entries().iter().any(|entry| entry.width() == 256));
    for entry in directory.entries() {
        let decoded = entry.decode().unwrap();
        assert_eq!(&decoded.rgba_data()[..4], &[255, 0, 0, 128]);
        assert_eq!(entry.height(), entry.width());
    }
}

#[test]
fn invalid_and_non_square_svg_are_rejected() {
    assert!(render_assets(b"not SVG").is_err());
    let error = render_assets(br#"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="4"/>"#)
        .err()
        .unwrap();
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
}
