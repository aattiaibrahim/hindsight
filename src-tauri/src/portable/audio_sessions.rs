//! The list of sound outputs and which programs play on them (for diagnostics) is
//! Windows-only for now.
#![allow(dead_code)]

pub struct Output {
    pub name: String,
    pub default: bool,
    /// (program, playing right now)
    pub apps: Vec<(String, bool)>,
}

pub fn outputs() -> Vec<Output> {
    Vec::new()
}

pub fn report_lines(_outputs: &[Output]) -> Vec<String> {
    vec!["Sound outputs: not listed on this system yet.".into()]
}
