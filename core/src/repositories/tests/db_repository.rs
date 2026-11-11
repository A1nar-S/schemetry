use super::*;

fn budget(max_cells: usize, max_cell_bytes: usize, max_total_bytes: usize) -> LobBudget {
    LobBudget::new(LobCapture::On {
        max_cells,
        max_cell_bytes,
        max_total_bytes,
    })
    .unwrap()
}

#[test]
fn capture_off_has_no_budget() {
    assert!(LobBudget::new(LobCapture::Off).is_none());
    assert_eq!(LobCapture::default(), LobCapture::Off);
}

#[test]
fn cells_within_the_limits_are_kept() {
    let mut b = budget(10, 100, 1_000);
    assert_eq!(b.allowance(), Ok(100));
    b.record(0, 1, "BLOB", true, vec![1, 2, 3]);
    b.record(2, 0, "TEXT", false, b"hello".to_vec());
    assert_eq!(b.captured.cells.len(), 2);
    assert_eq!(b.captured.cells[0].bytes, vec![1, 2, 3]);
    assert_eq!((b.captured.cells[1].row, b.captured.cells[1].col), (2, 0));
    assert!(b.captured.skipped.is_empty());
}

#[test]
fn a_cell_over_the_per_cell_limit_is_skipped() {
    let mut b = budget(10, 4, 1_000);
    b.record(0, 0, "BLOB", true, vec![0; 5]);
    assert!(b.captured.cells.is_empty());
    assert_eq!(b.captured.skipped.len(), 1);
    assert!(b.captured.skipped[0].reason.contains("per-cell"));
    // A later cell that fits is still captured.
    b.record(1, 0, "BLOB", true, vec![0; 4]);
    assert_eq!(b.captured.cells.len(), 1);
}

#[test]
fn the_call_wide_byte_and_cell_limits_hold() {
    let mut b = budget(10, 100, 150);
    b.record(0, 0, "BLOB", true, vec![0; 100]);
    assert_eq!(b.allowance(), Ok(50));
    b.record(1, 0, "BLOB", true, vec![0; 60]);
    assert!(b.captured.skipped[0].reason.contains("per-call"));
    b.record(2, 0, "BLOB", true, vec![0; 50]);
    assert_eq!(b.captured.cells.len(), 2);
    assert!(b.allowance().is_err());

    let mut b = budget(1, 100, 1_000);
    b.record(0, 0, "CLOB", false, b"a".to_vec());
    b.record(1, 0, "CLOB", false, b"b".to_vec());
    assert_eq!(b.captured.cells.len(), 1);
    assert!(b.captured.skipped[0].reason.contains("1 cells"));
}
