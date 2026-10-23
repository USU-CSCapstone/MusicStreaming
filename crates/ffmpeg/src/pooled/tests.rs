use std::process::Stdio;

use super::*;

#[test]
fn a_child_whose_standard_output_is_not_piped_cannot_be_read() {
    let registry = Registry::start().unwrap();
    let mut command = Command::new("sleep");
    command.arg("30").stderr(Stdio::piped());
    let mut pooled = Pooled::spawn(&registry, &mut command).unwrap();
    let error = pooled.stdout().unwrap_err();
    assert_eq!(error.to_string(), "ffmpeg's standard output was not piped");
}

#[test]
fn finishing_twice_reads_nothing_more() {
    let registry = Registry::start().unwrap();
    let mut command = Command::new("true");
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut pooled = Pooled::spawn(&registry, &mut command).unwrap();
    pooled.finish().unwrap();
    pooled.finish().unwrap();
}
