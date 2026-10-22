//! What is recorded of a run: its refusals, each named once and the rest counted, and its
//! compute, whichever way it ended.

use super::{NAME_CHARS, Outcome, REFUSALS_NAMED, Refused, RunError};

/// An outcome that ended with `result`, refused `refused`, after `ticks` of compute.
fn outcome(result: Result<String, RunError>, refused: Refused, ticks: u64) -> Outcome {
    Outcome { result, refused, ticks, ..Outcome::failed(RunError::OutOfTime) }
}

#[test]
fn refusals_are_named_once_and_past_the_first_ten_only_counted() {
    let mut refused = Refused::default();
    refused.add("Network access");
    refused.add("Network access");
    let hosts: Vec<String> = (0..12).map(|n| format!("{n}.example")).collect();
    for host in &hosts {
        refused.add(host);
    }
    // Retrying a host it was refused by name is not counted again; one past the ten is.
    refused.add("0.example");
    refused.add("11.example");
    let mut named = vec!["Network access".to_owned()];
    named.extend(hosts[..REFUSALS_NAMED - 1].iter().cloned());
    assert_eq!(refused, Refused { named, more: 4 });
    assert!(refused.to_string().ends_with("8.example and 4 more attempts"), "{refused}");
}

#[test]
fn a_long_name_is_cut_short() {
    let mut refused = Refused::default();
    let long = format!("{}.example", "a".repeat(NAME_CHARS));
    refused.add(&long);
    refused.add(&format!("{long}.other"));
    assert_eq!(refused.named, ["a".repeat(NAME_CHARS)], "kept short, and once");
}

#[test]
fn a_successful_run_reports_its_refusals_and_compute() {
    let mut refused = Refused::default();
    refused.add("Network access");
    refused.add("a.example");
    let ran = outcome(Ok("Saved 3.".into()), refused, 42);
    assert_eq!(
        ran.report(),
        "Saved 3. (refused Network access, a.example; about 42 ms of compute)"
    );
    assert_eq!(ran.summary(), "Saved 3.", "the summary is the plugin's own");
}

#[test]
fn compute_under_a_second_is_in_milliseconds() {
    let report = |ticks| outcome(Ok("done".into()), Refused::default(), ticks).report();
    assert_eq!(report(999), "done (about 999 ms of compute)");
    assert_eq!(report(1000), "done (about 1.0 s of compute)");
}

#[test]
fn a_failed_run_reports_its_compute() {
    let failed = outcome(Err(RunError::OutOfCompute), Refused::default(), 60_000);
    let summary = "the plugin ran past its compute budget";
    assert_eq!(failed.report(), format!("{summary} (about 60.0 s of compute)"));
    let mut refused = Refused::default();
    refused.add("x.example");
    refused.add("y.example");
    refused.more = 1;
    let failed = outcome(Err(RunError::Reported("no".into())), refused, 1_250);
    assert_eq!(
        failed.report(),
        "the plugin reported an error: no \
         (refused x.example, y.example and 1 more attempt; about 1.2 s of compute)"
    );
}
