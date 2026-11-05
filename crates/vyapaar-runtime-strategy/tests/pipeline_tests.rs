//! Integration tests for the declarative step pipeline.

use vyapaar_runtime_strategy::StepPipeline;

#[test]
fn test_step_pipeline_parse_and_validate() {
    let dsl = "signal -> size.fixed_fractional -> risk -> execute";
    let pipeline = StepPipeline::parse(dsl).expect("valid pipeline parse");
    assert_eq!(pipeline.steps.len(), 4);
    assert_eq!(pipeline.steps[0], "signal");
    assert_eq!(pipeline.steps[1], "size.fixed_fractional");
    assert_eq!(pipeline.steps[2], "risk");
    assert_eq!(pipeline.steps[3], "execute");
    assert!(pipeline.has_sizing_step());
}

#[test]
fn test_step_pipeline_invalid_ordering() {
    let dsl = "risk -> signal -> execute";
    let res = StepPipeline::parse(dsl);
    assert!(res.is_err());
}
