use crate::stdio_tailer::read_new_lines;
use runfile_parser::StdioStream;
use std::io::Write;
use std::path::PathBuf;

#[test]
fn read_new_lines_advances_past_invalid_utf8() {
	// Audit L10: an invalid-UTF-8 line followed by a valid one. The byte-based
	// tailer must consume the whole file (advancing the offset) instead of
	// stalling at the bad byte forever, as the old `read_line`-into-`String`
	// path did (it errored and left the position unadvanced).
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("log.txt");
	let mut f = std::fs::File::create(&path).unwrap();
	f.write_all(b"bad\xFFline\ngood line\n").unwrap();
	f.flush().unwrap();

	let path_buf: PathBuf = path.clone();
	let mut line_buf = String::new();
	let new_pos = read_new_lines(&path_buf, 0, &mut line_buf, &StdioStream::Stdout);

	let file_len = std::fs::metadata(&path).unwrap().len();
	assert_eq!(new_pos, file_len, "tailer must advance past the invalid-UTF-8 line");
	assert!(
		line_buf.is_empty(),
		"both lines were complete; nothing should be buffered"
	);
}
