use vyapaar_runtime_enhancer::DSRCalculator;

#[test]
fn dsr_calculator_returns_sharpe_below_minimum_sample() {
    let calc = DSRCalculator::new(10);
    // Fewer than 30 returns: DSR falls back to the raw Sharpe ratio.
    let returns = vec![0.01, 0.02, -0.01, 0.03];
    assert_eq!(calc.compute(1.5, &returns), 1.5);
}
