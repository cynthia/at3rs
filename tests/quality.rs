use at3rs::eval;

#[test]
#[ignore = "expensive quality-regression gate"]
fn quality_regression_test_fixture() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let metrics =
        eval::evaluate_atrac3_roundtrip_file(format!("{manifest_dir}/fixtures/test.wav"), 2, 384)
            .unwrap();

    assert!(
        metrics.snr_db >= 10.0,
        "test.wav SNR regressed: {metrics:?}"
    );
    assert!(
        metrics.rmse <= 700.0,
        "test.wav RMSE regressed: {metrics:?}"
    );
}

#[test]
#[ignore = "expensive quality-regression gate"]
fn quality_regression_noise_fixture() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let metrics =
        eval::evaluate_atrac3_roundtrip_file(format!("{manifest_dir}/fixtures/noise.wav"), 2, 384)
            .unwrap();

    assert!(
        metrics.snr_db >= 2.5,
        "noise.wav SNR regressed: {metrics:?}"
    );
    assert!(
        metrics.rmse <= 10_100.0,
        "noise.wav RMSE regressed: {metrics:?}"
    );
}
