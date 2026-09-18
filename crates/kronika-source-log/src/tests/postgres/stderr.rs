use super::{PARTS, Part, find_marker};

#[test]
fn marker_scan_returns_the_first_recognized_marker() {
    let line = "prefix HINT:  quoted DETAIL:  later";
    let (at, marker, part) = find_marker(line, PARTS).expect("recognized marker");

    assert_eq!(line.get(at..at + marker.len()), Some("HINT:  "));
    assert_eq!(part, Part::Hint);
}
