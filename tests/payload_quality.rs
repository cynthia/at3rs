use at3rs::eval;
use std::fs;

struct PayloadBaseline {
    path: &'static str,
    min_snr_db: f64,
    max_rmse: f64,
}

const PAYLOAD_BASELINES: &[PayloadBaseline] = &[
    PayloadBaseline {
        path: "fixtures/billiejean_30s.wav",
        min_snr_db: 4.0,
        max_rmse: 1_900.0,
    },
    PayloadBaseline {
        path: "fixtures/iwish_30s.wav",
        min_snr_db: 9.2,
        max_rmse: 3_000.0,
    },
    PayloadBaseline {
        path: "fixtures/magnetic_30s.wav",
        min_snr_db: 8.6,
        max_rmse: 4_500.0,
    },
    PayloadBaseline {
        path: "fixtures/secretgarden_30s.wav",
        min_snr_db: 8.9,
        max_rmse: 3_500.0,
    },
    PayloadBaseline {
        path: "fixtures/walkmehome_30s.wav",
        min_snr_db: 9.2,
        max_rmse: 3_350.0,
    },
];

#[test]
fn fixture_wavs_are_flat() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let fixtures_dir = format!("{manifest_dir}/fixtures");

    for entry in fs::read_dir(&fixtures_dir).expect("fixtures directory should exist") {
        let entry = entry.expect("fixtures entry should be readable");
        let path = entry.path();
        if path.is_dir() {
            let has_nested_wav = fs::read_dir(&path)
                .expect("nested fixture directory should be readable")
                .any(|nested| {
                    nested
                        .expect("nested fixture entry should be readable")
                        .path()
                        .extension()
                        .is_some_and(|ext| ext == "wav")
                });
            assert!(
                !has_nested_wav,
                "fixture WAVs must be flat: {}",
                path.display()
            );
        }
    }
}

#[test]
fn baseline_iwish_fixture_snr() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let path = format!("{manifest_dir}/fixtures/iwish_30s.wav");
    let metrics = eval::evaluate_atrac3_roundtrip_file(&path, 2, 384)
        .unwrap_or_else(|err| panic!("failed to evaluate baseline fixture: {err}"));

    assert!(
        metrics.snr_db >= 18.0,
        "iwish_30s.wav SNR regressed: {metrics:?}"
    );
    assert!(
        metrics.rmse <= 3_000.0,
        "iwish_30s.wav RMSE regressed: {metrics:?}"
    );
}

#[test]
#[ignore = "expensive payload-regression gate"]
fn quality_regression_payload_fixtures() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");

    for payload in PAYLOAD_BASELINES {
        let path = format!("{manifest_dir}/{}", payload.path);
        let metrics = eval::evaluate_atrac3_roundtrip_file(&path, 2, 384)
            .unwrap_or_else(|err| panic!("failed to evaluate {}: {err}", payload.path));

        println!(
            "{}: snr_db={:.2} rmse={:.2} offset={}",
            payload.path, metrics.snr_db, metrics.rmse, metrics.best_offset
        );

        assert!(
            metrics.snr_db >= payload.min_snr_db,
            "{} SNR regressed: {metrics:?}",
            payload.path
        );
        assert!(
            metrics.rmse <= payload.max_rmse,
            "{} RMSE regressed: {metrics:?}",
            payload.path
        );
    }
}
