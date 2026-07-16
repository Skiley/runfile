use crate::cmd_utilities::INIT_TEMPLATE;

#[test]
fn init_template_parses_as_runfile() {
	// The template is a literal string, so the type system can't catch
	// typos. Round-trip it through the parser to make sure `:init`
	// always produces a file the rest of the toolchain accepts.
	let runfile = runfile_parser::parse_runfile(INIT_TEMPLATE).expect("init template must parse");
	assert!(runfile.targets.contains_key("hello"));
}
