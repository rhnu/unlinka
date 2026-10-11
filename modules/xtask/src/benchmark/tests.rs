use super::{Sample, median, summarize};

#[test]
fn median_handles_odd_and_even_round_counts() {
    assert_eq!(median(vec![70.0, 10.0, 30.0]), 30.0);
    assert_eq!(median(vec![60.0, 10.0, 30.0, 20.0]), 25.0);
}

#[test]
fn summaries_keep_shapes_and_methods_separate() {
    let sample = |shape: &str, method: &str, operation, wall| Sample {
        shape: shape.to_owned(),
        round: 1,
        method: method.to_owned(),
        files: 1000,
        file_bytes: 1024,
        directories: 1,
        operation_milliseconds: operation,
        wall_milliseconds: wall,
        exit_code: 0,
        target_absent: true,
        sentinel_unchanged: true,
        stderr: String::new(),
    };
    let samples = [
        sample("flat", "unlinka-1", 10.0, 40.0),
        sample("flat", "unlinka-1", 30.0, 60.0),
        sample("tree", "unlinka-1", 100.0, 200.0),
        sample("flat", "powershell", 50.0, 150.0),
    ];
    let rows = summarize(&samples, &["flat".to_owned(), "tree".to_owned()], 1000);
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].rounds, 2);
    assert_eq!(rows[0].median_operation_milliseconds, 20.0);
    assert_eq!(rows[0].median_wall_milliseconds, 50.0);
    assert_eq!(rows[0].files_per_second, 50000.0);
    assert_eq!(rows[1].method, "powershell");
    assert_eq!(rows[1].median_operation_milliseconds, 50.0);
    assert_eq!(rows[2].shape, "tree");
    assert_eq!(rows[2].median_operation_milliseconds, 100.0);
}
